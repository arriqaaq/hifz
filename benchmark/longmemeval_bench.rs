//! LongMemEval-S Benchmark for hifz (Rust + SurrealDB)
//!
//! Evaluates retrieval recall on the LongMemEval-S dataset (ICLR 2025).
//! Each question builds a fresh in-memory SurrealDB, indexes ~48 sessions,
//! runs hybrid search (BM25 + HNSW vector + RRF), and checks if the gold
//! session IDs appear in the top-K results.
//!
//! Usage:
//!   # Download dataset first:
//!   pip install huggingface_hub
//!   python3 -c "
//!   from huggingface_hub import hf_hub_download
//!   hf_hub_download(repo_id='xiaowu0162/longmemeval-cleaned',
//!                   filename='longmemeval_s_cleaned.json',
//!                   repo_type='dataset', local_dir='benchmark/data')
//!   "
//!
//!   # Run benchmark:
//!   cargo run --bin longmemeval-bench -- [bm25|hybrid]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::local::Mem;
use surrealdb::types::SurrealValue;

use hifz::embed::Embedder;

// --- Dataset types ---

#[derive(Debug, Deserialize)]
struct LongMemEvalEntry {
    question_id: String,
    question_type: String,
    question: String,
    #[allow(dead_code)]
    question_date: Option<String>,
    #[allow(dead_code)]
    answer: Option<serde_json::Value>,
    answer_session_ids: Vec<String>,
    #[allow(dead_code)]
    haystack_dates: Option<Vec<String>>,
    haystack_session_ids: Vec<String>,
    haystack_sessions: Vec<Vec<Turn>>,
}

#[derive(Debug, Deserialize)]
struct Turn {
    role: String,
    content: String,
    #[allow(dead_code)]
    has_answer: Option<bool>,
}

// --- Result types ---

#[derive(Debug, Serialize)]
struct BenchResult {
    question_id: String,
    question_type: String,
    recall_any_at_5: f64,
    recall_any_at_10: f64,
    recall_any_at_20: f64,
    ndcg_at_10: f64,
    mrr: f64,
    retrieved_session_ids: Vec<String>,
    gold_session_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BenchSummary {
    mode: String,
    questions: usize,
    recall_any_at_5: f64,
    recall_any_at_10: f64,
    recall_any_at_20: f64,
    ndcg_at_10: f64,
    mrr: f64,
    per_type: HashMap<String, TypeSummary>,
    per_question: Vec<BenchResult>,
}

#[derive(Debug, Serialize)]
struct TypeSummary {
    count: usize,
    recall_any_at_5: f64,
    recall_any_at_10: f64,
}

// --- Scoring functions ---

fn recall_any(retrieved: &[String], gold: &[String], k: usize) -> f64 {
    let top_k: HashSet<&str> = retrieved.iter().take(k).map(|s| s.as_str()).collect();
    if gold.iter().any(|g| top_k.contains(g.as_str())) {
        1.0
    } else {
        0.0
    }
}

fn dcg(relevances: &[bool], k: usize) -> f64 {
    let mut sum = 0.0;
    for (i, &rel) in relevances.iter().take(k).enumerate() {
        if rel {
            sum += 1.0 / (i as f64 + 2.0).log2();
        }
    }
    sum
}

fn ndcg_score(retrieved: &[String], gold: &HashSet<String>, k: usize) -> f64 {
    let rels: Vec<bool> = retrieved
        .iter()
        .take(k)
        .map(|id| gold.contains(id))
        .collect();
    let ideal_count = k.min(gold.len());
    let ideal_rels: Vec<bool> = (0..ideal_count).map(|_| true).collect();
    let ideal_dcg = dcg(&ideal_rels, k);
    if ideal_dcg == 0.0 {
        return 0.0;
    }
    dcg(&rels, k) / ideal_dcg
}

fn mrr_score(retrieved: &[String], gold: &HashSet<String>) -> f64 {
    for (i, id) in retrieved.iter().enumerate() {
        if gold.contains(id) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

/// Truncate a string at a char boundary, never splitting a multi-byte char.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn chunk_session_to_text(turns: &[Turn]) -> String {
    turns
        .iter()
        .map(|t| format!("{}: {}", t.role, t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- Schema for per-question in-memory DB ---

// Phase A: table + fields only (before inserts)
// One `content` field with full concatenated text (title + narrative)
const BENCH_SCHEMA_TABLES: &str = r#"
DEFINE TABLE IF NOT EXISTS observation SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS content    ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS embedding  ON observation TYPE option<array<float>>
"#;

// Phase B: analyzer + single BM25 index on content (AFTER all inserts)
// Single index over the full text, snowball(english) stemming
const BENCH_SCHEMA_FT: &str = r#"
DEFINE ANALYZER IF NOT EXISTS obs_analyzer TOKENIZERS blank, class
  FILTERS lowercase, snowball(english);
DEFINE INDEX IF NOT EXISTS obs_content_ft ON TABLE observation
  FIELDS content FULLTEXT ANALYZER obs_analyzer BM25
"#;

// HNSW vector index (can be defined before or after inserts — incremental)
const BENCH_SCHEMA_VEC: &str = r#"
DEFINE INDEX IF NOT EXISTS obs_vec ON TABLE observation
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE
"#;

// No sanitize_query needed — bind variables handle escaping, and SurrealDB's
// tokenizer handles punctuation natively.

/// Run BM25-only search on the in-memory DB.
/// Uses inline subqueries (no LET) because multi-statement queries don't return
/// results in the SurrealDB 3.1.0-alpha embedded SDK.
/// BM25 search on single `content` field.
async fn search_bm25(db: &Surreal<Db>, query: &str, limit: usize) -> Result<Vec<String>> {
    // Single BM25 index on content, OR semantics, bind variable
    let sql = format!(
        "SELECT id, session_id, search::score(1) AS ft_score \
         FROM observation WHERE content @1,OR@ $q \
         ORDER BY ft_score DESC LIMIT {limit}"
    );
    let mut resp = db.query(&sql).bind(("q", query.to_string())).await?;

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<surrealdb::types::RecordId>,
        session_id: Option<String>,
        ft_score: Option<f64>,
    }
    let rows: Vec<Row> = resp.take(0)?;

    Ok(rows.into_iter().filter_map(|r| r.session_id).collect())
}

/// Run hybrid search (BM25 + vector + RRF) on the in-memory DB.
async fn search_hybrid(
    db: &Surreal<Db>,
    embedder: &Embedder,
    query: &str,
    limit: usize,
) -> Result<Vec<String>> {
    let query_vec = embedder.embed_single(query)?;
    // Two streams: vector + single BM25 on content, fused with RRF
    let sql = format!(
        "search::rrf([\
             (SELECT id, vector::distance::knn() AS distance \
              FROM observation WHERE embedding <|{limit},80|> $query_vec),\
             (SELECT id, search::score(1) AS ft_score \
              FROM observation WHERE content @1,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit})\
         ], {limit}, 60)"
    );
    let mut resp = db
        .query(&sql)
        .bind(("query_vec", query_vec))
        .bind(("q", query.to_string()))
        .await?;

    #[derive(Debug, SurrealValue)]
    struct RrfRow {
        id: Option<surrealdb::types::RecordId>,
        rrf_score: Option<f64>,
    }
    let fused: Vec<RrfRow> = resp.take(0)?;
    if fused.is_empty() {
        return Ok(vec![]);
    }

    let ids: Vec<surrealdb::types::RecordId> = fused.iter().filter_map(|r| r.id.clone()).collect();
    let mut fetch = db
        .query("SELECT id, session_id FROM observation WHERE id IN $ids")
        .bind(("ids", ids))
        .await?;

    #[derive(Debug, SurrealValue)]
    struct ObsRow {
        id: Option<surrealdb::types::RecordId>,
        session_id: Option<String>,
    }
    let rows: Vec<ObsRow> = fetch.take(0)?;

    #[allow(clippy::mutable_key_type)]
    let sid_map: HashMap<surrealdb::types::RecordId, String> = rows
        .into_iter()
        .filter_map(|r| Some((r.id?, r.session_id?)))
        .collect();

    let ordered: Vec<String> = fused
        .iter()
        .filter_map(|r| sid_map.get(r.id.as_ref()?).cloned())
        .collect();

    Ok(ordered)
}

/// Execute a semicolon-delimited SQL block.
async fn exec_sql_block(db: &Surreal<Db>, block: &str) -> Result<()> {
    for stmt in block
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.starts_with("--"))
    {
        db.query(&format!("{stmt};")).await?.check()?;
    }
    Ok(())
}

/// Phase A: create table + fields (call BEFORE inserts). Caller must set ns/db.
async fn init_bench_tables_raw(db: &Surreal<Db>) -> Result<()> {
    exec_sql_block(db, BENCH_SCHEMA_TABLES).await
}

/// HNSW vector index (incremental, safe before inserts).
async fn init_bench_vec_index(db: &Surreal<Db>) -> Result<()> {
    exec_sql_block(db, BENCH_SCHEMA_VEC).await
}

/// Phase B: create analyzer + BM25 FULLTEXT indexes (call AFTER all inserts).
async fn init_bench_ft_indexes_raw(db: &Surreal<Db>) -> Result<()> {
    exec_sql_block(db, BENCH_SCHEMA_FT).await
}

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(64 * 1024 * 1024)
        .build()?;
    runtime.block_on(run())
}

async fn run() -> Result<()> {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "bm25".to_string());
    let use_vectors = mode != "bm25";

    println!("=== LongMemEval-S Benchmark ({mode}) ===\n");

    // Load dataset
    let data_path = "benchmark/data/longmemeval_s_cleaned.json";
    if !Path::new(data_path).exists() {
        bail!(
            "Dataset not found at {data_path}\n\
             Download with:\n  \
             ./benchmark/download_dataset.sh"
        );
    }

    println!("Loading dataset...");
    let raw_json = fs::read_to_string(data_path)?;
    let all_entries: Vec<LongMemEvalEntry> = serde_json::from_str(&raw_json)?;

    // Filter out abstention questions
    let abstention_suffixes = ["_abs"];
    let entries: Vec<&LongMemEvalEntry> = all_entries
        .iter()
        .filter(|e| {
            !abstention_suffixes
                .iter()
                .any(|s| e.question_type.ends_with(s))
        })
        .collect();
    println!(
        "Loaded {} questions ({} abstention excluded)\n",
        entries.len(),
        all_entries.len() - entries.len()
    );

    // Initialize embedder if needed
    let embedder = if use_vectors {
        println!("Loading embedding model (all-MiniLM-L6-v2, 384 dims)...");
        Some(Arc::new(Embedder::new()?))
    } else {
        None
    };

    let pb = ProgressBar::new(entries.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("  {bar:50.green/black} {pos}/{len} ({eta}) R@5: {msg}")
            .unwrap(),
    );

    let mut results: Vec<BenchResult> = Vec::with_capacity(entries.len());
    let start = Instant::now();

    for (q_idx, entry) in entries.iter().enumerate() {
        // Create a fresh in-memory SurrealDB for each question
        // Use a unique db name per question to avoid session reuse issues
        let db = Surreal::new::<Mem>(()).await?;
        let db_name = format!("q{q_idx}");
        db.use_ns("bench").use_db(&db_name).await?;

        // Phase A: table + fields (before inserts)
        init_bench_tables_raw(&db).await?;
        if use_vectors {
            init_bench_vec_index(&db).await?;
        }

        // Insert all haystack sessions
        for (i, turns) in entry.haystack_sessions.iter().enumerate() {
            let session_id = &entry.haystack_session_ids[i];
            let text = chunk_session_to_text(turns);

            // content = full text (title slice + full narrative)
            let content = text.clone();

            let embedding = if let Some(ref emb) = embedder {
                let embed_text = truncate_str(&text, 512);
                emb.embed_single(embed_text).ok()
            } else {
                None
            };

            db.query(
                "CREATE observation SET \
                 session_id = $session_id, \
                 content = $content, \
                 embedding = $embedding",
            )
            .bind(("session_id", session_id.clone()))
            .bind(("content", content))
            .bind(("embedding", embedding))
            .await?
            .check()?;
        }

        // Phase B: create BM25 indexes AFTER all data is inserted
        init_bench_ft_indexes_raw(&db).await?;

        // Search
        let retrieved = if use_vectors {
            search_hybrid(&db, embedder.as_ref().unwrap(), &entry.question, 20).await?
        } else {
            search_bm25(&db, &entry.question, 20).await?
        };

        let gold_set: HashSet<String> = entry.answer_session_ids.iter().cloned().collect();

        let result = BenchResult {
            question_id: entry.question_id.clone(),
            question_type: entry.question_type.clone(),
            recall_any_at_5: recall_any(&retrieved, &entry.answer_session_ids, 5),
            recall_any_at_10: recall_any(&retrieved, &entry.answer_session_ids, 10),
            recall_any_at_20: recall_any(&retrieved, &entry.answer_session_ids, 20),
            ndcg_at_10: ndcg_score(&retrieved, &gold_set, 10),
            mrr: mrr_score(&retrieved, &gold_set),
            retrieved_session_ids: retrieved.into_iter().take(10).collect(),
            gold_session_ids: entry.answer_session_ids.clone(),
        };
        results.push(result);

        // Update progress
        let running_r5 =
            results.iter().map(|r| r.recall_any_at_5).sum::<f64>() / results.len() as f64;
        pb.set_message(format!("{:.1}%", running_r5 * 100.0));
        pb.inc(1);
    }

    pb.finish_and_clear();
    let elapsed = start.elapsed();

    // Compute aggregates
    let n = results.len() as f64;
    let avg_r5 = results.iter().map(|r| r.recall_any_at_5).sum::<f64>() / n;
    let avg_r10 = results.iter().map(|r| r.recall_any_at_10).sum::<f64>() / n;
    let avg_r20 = results.iter().map(|r| r.recall_any_at_20).sum::<f64>() / n;
    let avg_ndcg = results.iter().map(|r| r.ndcg_at_10).sum::<f64>() / n;
    let avg_mrr = results.iter().map(|r| r.mrr).sum::<f64>() / n;

    println!("\n=== LongMemEval-S Results ({mode}) ===");
    println!("Questions:     {}", results.len());
    println!(
        "Time:          {:.1}s ({:.1}ms/question)",
        elapsed.as_secs_f64(),
        elapsed.as_millis() as f64 / n
    );
    println!("recall_any@5:  {:.1}%", avg_r5 * 100.0);
    println!("recall_any@10: {:.1}%", avg_r10 * 100.0);
    println!("recall_any@20: {:.1}%", avg_r20 * 100.0);
    println!("NDCG@10:       {:.1}%", avg_ndcg * 100.0);
    println!("MRR:           {:.1}%", avg_mrr * 100.0);

    // Per-type breakdown
    let mut by_type: HashMap<String, Vec<&BenchResult>> = HashMap::new();
    for r in &results {
        by_type.entry(r.question_type.clone()).or_default().push(r);
    }

    println!("\nBy question type:");
    let mut type_summary: HashMap<String, TypeSummary> = HashMap::new();
    for (qtype, type_results) in &by_type {
        let tn = type_results.len() as f64;
        let r5 = type_results.iter().map(|r| r.recall_any_at_5).sum::<f64>() / tn;
        let r10 = type_results.iter().map(|r| r.recall_any_at_10).sum::<f64>() / tn;
        println!(
            "  {:<30} R@5: {:5.1}%  R@10: {:5.1}%  (n={})",
            qtype,
            r5 * 100.0,
            r10 * 100.0,
            type_results.len()
        );
        type_summary.insert(
            qtype.clone(),
            TypeSummary {
                count: type_results.len(),
                recall_any_at_5: r5,
                recall_any_at_10: r10,
            },
        );
    }

    // Save results
    let summary = BenchSummary {
        mode: mode.clone(),
        questions: results.len(),
        recall_any_at_5: avg_r5,
        recall_any_at_10: avg_r10,
        recall_any_at_20: avg_r20,
        ndcg_at_10: avg_ndcg,
        mrr: avg_mrr,
        per_type: type_summary,
        per_question: results,
    };

    let out_path = format!("benchmark/data/longmemeval_results_{mode}_rust.json");
    fs::write(&out_path, serde_json::to_string_pretty(&summary)?)?;
    println!("\nResults saved to {out_path}");

    Ok(())
}
