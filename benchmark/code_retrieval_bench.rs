//! code-retrieval correctness diagnosis: **does hifz `search_code` find the
//! right function**, and when it doesn't, **why** — localized into a
//! recoverable (ranking/pool) vs deep (representation) failure.
//!
//! Retrieval correctness is the headline. Token cost is a single demoted,
//! correctness-conditioned line — saving tokens on a wrong retrieval is
//! negative value, not a win. grep is the token-cost baseline only.
//!
//! Oracle = docstring→code (copied verbatim from `benchmark/corpus_code_bench.rs`,
//! left untouched; this bench is additive). The paraphrase style (identifiers
//! stripped) is the semantic test the gate lives on; raw is a sanity row.
//!
//! Diagnose-only: NO retrieval/library code is changed here. The localized fix
//! is the documented next step.
//!
//! Usage:
//!   cargo run --release --bin code-retrieval-bench
//!   cargo run --release --bin code-retrieval-bench -- --root . --project coderetr

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use hifz::code::index::{IndexOpts, index_repo};
use hifz::code::search::{CodeSearchOpts, CodeSearchResult, search_code};
use hifz::db::{self, Db, init_schema};
use hifz::embed::Embedder;
use hifz::models::RrfResult;
use kernel::code_parse::walker::{WalkOpts, walk};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

#[path = "corpus_common.rs"]
mod corpus_common;
use corpus_common::Verdict;

fn est_tokens(s: &str) -> usize {
    (s.len() / 4).max(1)
}

const MIN_DOCUMENTED: usize = 25;
/// Agent-budget top-k the headline correctness number is measured at.
const K: usize = 10;
/// Wide pool used to localize a miss: if the oracle appears within WIDE but not
/// K, the candidate pool / fusion buried it (recoverable). If absent even at
/// WIDE, neither signal surfaces it (deep representation failure).
const WIDE: usize = 200;

// ---------------------------------------------------------------------------
// Oracle helpers — copied verbatim from benchmark/corpus_code_bench.rs
// (that file is intentionally NOT modified).
// ---------------------------------------------------------------------------

#[derive(Debug, SurrealValue, Clone)]
struct SymRow {
    name: Option<String>,
    qualified: Option<String>,
    kind: Option<String>,
    doc: Option<String>,
    path: Option<String>,
    start_line: Option<i64>,
    end_line: Option<i64>,
    signature: Option<String>,
}

/// Recover the doc comment from source — contiguous `///`/`//!`/`#`/`*` lines
/// immediately above the definition (`code_symbol.doc` is reserved but never
/// populated by intel).
fn doc_above(root: &str, path: &str, start_line: i64) -> Option<String> {
    if start_line < 2 {
        return None;
    }
    let txt = std::fs::read_to_string(std::path::Path::new(root).join(path)).ok()?;
    let lines: Vec<&str> = txt.lines().collect();
    let mut j = (start_line as usize).saturating_sub(2);
    let mut collected: Vec<String> = Vec::new();
    loop {
        let raw = lines.get(j)?.trim();
        if raw.is_empty() || raw.starts_with("#[") || raw.starts_with("#!") {
            // attribute / blank between doc and def — skip upward
        } else if raw.starts_with("///")
            || raw.starts_with("//!")
            || raw.starts_with("//")
            || raw.starts_with('*')
            || raw.starts_with("/**")
            || raw.starts_with("\"\"\"")
            || raw.starts_with('#')
        {
            collected.push(raw.to_string());
        } else {
            break;
        }
        if j == 0 {
            break;
        }
        j -= 1;
    }
    if collected.is_empty() {
        return None;
    }
    collected.reverse();
    Some(collected.join("\n"))
}

struct Probe {
    raw: String,
    paraphrase: Option<String>,
    path: String,
    start: i64,
    end: i64,
    /// 1-indexed first line of the doc block above `start` (else `start`).
    /// `code_symbol.start_line` is the *definition node* line and EXCLUDES the
    /// `///` doc the query is derived from (verified: graph.rs:149). The
    /// honest oracle span for "did the agent get this function" is
    /// `[doc_top .. end]` — the function *plus its doc* (what the query is
    /// about and what is indexed). The strict `[start..end]` span is kept only
    /// to report the auditable correction delta, never to flatter.
    doc_top: i64,
}

/// 1-indexed topmost line of the contiguous doc/attr block immediately above
/// `start_line` (mirrors `doc_above`'s upward scan); `start_line` if none.
fn doc_top(root: &str, path: &str, start_line: i64) -> i64 {
    if start_line < 2 {
        return start_line;
    }
    let Ok(txt) = std::fs::read_to_string(std::path::Path::new(root).join(path)) else {
        return start_line;
    };
    let lines: Vec<&str> = txt.lines().collect();
    let mut j = (start_line as usize).saturating_sub(2);
    let mut top: Option<usize> = None;
    loop {
        let Some(raw) = lines.get(j).map(|s| s.trim()) else {
            break;
        };
        if raw.is_empty() || raw.starts_with("#[") || raw.starts_with("#!") {
            // attribute / blank between doc and def — keep scanning upward
        } else if raw.starts_with("///")
            || raw.starts_with("//!")
            || raw.starts_with("//")
            || raw.starts_with('*')
            || raw.starts_with("/**")
            || raw.starts_with("\"\"\"")
            || raw.starts_with('#')
        {
            top = Some(j);
        } else {
            break;
        }
        if j == 0 {
            break;
        }
        j -= 1;
    }
    match top {
        Some(t) => (t as i64) + 1, // 0-indexed → 1-indexed
        None => start_line,
    }
}

/// First natural-language sentence of a doc comment, comment markers stripped.
fn first_sentence(doc: &str) -> String {
    let cleaned: String = doc
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches("///")
                .trim_start_matches("//!")
                .trim_start_matches("//")
                .trim_start_matches('*')
                .trim_start_matches('#')
                .trim()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let end = cleaned
        .find(". ")
        .map(|i| i + 1)
        .unwrap_or(cleaned.len())
        .min(cleaned.len());
    cleaned[..end].trim().to_string()
}

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_lowercase())
        .collect()
}

/// Identifier-stripped paraphrase: drop tokens matching the symbol name, any
/// `::`-qualified segment, or any identifier in the signature. Deterministic.
fn paraphrase(sentence: &str, sym: &SymRow) -> Option<String> {
    let mut ban: HashSet<String> = HashSet::new();
    if let Some(n) = &sym.name {
        ban.extend(tokens(n));
    }
    if let Some(q) = &sym.qualified {
        for seg in q.split("::") {
            ban.extend(tokens(seg));
        }
    }
    if let Some(sig) = &sym.signature {
        ban.extend(tokens(sig));
    }
    let kept: Vec<String> = tokens(sentence)
        .into_iter()
        .filter(|t| !ban.contains(t))
        .collect();
    if kept.len() < 3 {
        return None;
    }
    Some(kept.join(" "))
}

// ---------------------------------------------------------------------------
// Oracle matching — EXACT repo-relative path (paths are identical `f.rel`
// form everywhere; the basename fallback only adds cross-crate collisions).
// ---------------------------------------------------------------------------

/// File-level: did the agent get the right *file* into context (the same
/// question grep answers — the honest, grep-comparable correctness metric).
fn file_rank(hits: &[hifz::code::search::CodeSearchResult], p: &Probe) -> Option<usize> {
    hits.iter().position(|h| h.path == p.path)
}

/// CORRECTED chunk-rank: a returned chunk overlaps `[doc_top .. end]` (the
/// function + its doc — what the query is about and what is indexed). This is
/// the metric that answers "did the agent actually get this function".
fn chunk_rank(hits: &[hifz::code::search::CodeSearchResult], p: &Probe) -> Option<usize> {
    hits.iter().position(|h| {
        h.path == p.path && (h.start_line as i64) <= p.end && p.doc_top <= (h.end_line as i64)
    })
}

/// STRICT chunk-rank: overlaps the bare definition span `[start..end]`
/// (excludes the doc). Used ONLY to report how many corrected-hits the
/// doc-excluding oracle would have wrongly missed — an auditable delta, never
/// the headline.
fn chunk_rank_strict(hits: &[hifz::code::search::CodeSearchResult], p: &Probe) -> Option<usize> {
    hits.iter().position(|h| {
        h.path == p.path && (h.start_line as i64) <= p.end && p.start <= (h.end_line as i64)
    })
}

async fn run_search(
    db: &Surreal<Db>,
    emb: &Embedder,
    project: &str,
    q: &str,
    limit: usize,
) -> Vec<hifz::code::search::CodeSearchResult> {
    let opts = CodeSearchOpts {
        limit,
        project: Some(project.to_string()),
        ..Default::default()
    };
    search_code(db, emb, q, &opts).await.unwrap_or_default()
}

/// BM25-only, wide — does the oracle file surface via the lexical signal alone
/// in a wide pool? (Raw FT branch, copied from corpus_code_bench.rs:181-208.)
async fn bm25_file_present(
    db: &Surreal<Db>,
    project: &str,
    q: &str,
    limit: usize,
    p: &Probe,
) -> bool {
    #[derive(Debug, SurrealValue)]
    struct H {
        path: Option<String>,
        ft: Option<f64>,
    }
    let sql = "SELECT path, search::score(1) AS ft FROM code_chunk \
               WHERE content @1@ $q AND (project = $p OR project = 'global') \
               ORDER BY ft DESC LIMIT $l";
    let mut r = match db
        .query(sql)
        .bind(("q", q.to_string()))
        .bind(("p", project.to_string()))
        .bind(("l", limit as i64))
        .await
    {
        Ok(r) => r,
        Err(_) => return false,
    };
    let rows: Vec<H> = r.take(0).unwrap_or_default();
    rows.iter()
        .any(|h| h.path.as_deref() == Some(p.path.as_str()))
}

#[derive(Debug, SurrealValue)]
struct CRow {
    id: Option<RecordId>,
    path: Option<String>,
    language: Option<String>,
    content: Option<String>,
    start_line: Option<i64>,
    end_line: Option<i64>,
    distance: Option<f64>,
    ft_score: Option<f64>,
}

fn crow_to_result(r: CRow, score: f64, via: &'static str) -> CodeSearchResult {
    CodeSearchResult {
        id: String::new(),
        file_id: String::new(),
        path: r.path.unwrap_or_default(),
        language: r.language.unwrap_or_default(),
        start_line: r.start_line.unwrap_or(0).max(0) as usize,
        end_line: r.end_line.unwrap_or(0).max(0) as usize,
        snippet: r.content.unwrap_or_default(),
        score,
        via,
    }
}

/// A1 — EXACT replica of `search_code`'s two-branch `max(1-d, ft/4)` fusion
/// (src/code/search.rs:87-200) but with a WIDE per-branch candidate pool,
/// truncated to `k`. Isolates "is the ≤limit pool alone the bottleneck?".
/// Production `search_code` is NOT modified — this is bench-local.
async fn arm_widen_max(
    db: &Surreal<Db>,
    emb: &Embedder,
    project: &str,
    q: &str,
    wide: usize,
    k: usize,
) -> Vec<CodeSearchResult> {
    let Ok(qv) = emb.embed_single(q) else {
        return vec![];
    };
    let vsql = format!(
        "SELECT id, path, language, content, start_line, end_line, \
         vector::distance::knn() AS distance FROM code_chunk \
         WHERE embedding <|{wide},80|> $qv AND (project=$p OR project='global')"
    );
    let fsql = format!(
        "SELECT id, path, language, content, start_line, end_line, \
         search::score(1) AS ft_score FROM code_chunk \
         WHERE content @1@ $q AND (project=$p OR project='global') \
         ORDER BY ft_score DESC LIMIT {wide}"
    );
    let vrows: Vec<CRow> = match db
        .query(&vsql)
        .bind(("qv", qv.to_vec()))
        .bind(("p", project.to_string()))
        .await
    {
        Ok(mut r) => r.take(0).unwrap_or_default(),
        Err(_) => vec![],
    };
    let frows: Vec<CRow> = match db
        .query(&fsql)
        .bind(("q", q.to_string()))
        .bind(("p", project.to_string()))
        .await
    {
        Ok(mut r) => r.take(0).unwrap_or_default(),
        Err(_) => vec![],
    };
    struct M {
        rec: CRow,
        score: f64,
    }
    let mut by: HashMap<String, M> = HashMap::new();
    for h in vrows {
        let key = format!("{:?}", h.id);
        let score = h.distance.map(|d| (1.0 - d).clamp(0.0, 1.0)).unwrap_or(0.0);
        by.entry(key).or_insert(M { rec: h, score });
    }
    for h in frows {
        let key = format!("{:?}", h.id);
        let score = h.ft_score.map(|s| (s / 4.0).clamp(0.0, 1.0)).unwrap_or(0.0);
        by.entry(key)
            .and_modify(|m| {
                if score > m.score {
                    m.score = score;
                }
            })
            .or_insert(M { rec: h, score });
    }
    let mut out: Vec<M> = by.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(k);
    out.into_iter()
        .map(|m| crow_to_result(m.rec, m.score, "a1"))
        .collect()
}

/// A2 — WIDE pool + SurrealDB `search::rrf` fusion, the EXACT pattern hifz's
/// own memory search uses (src/search.rs:142-153), ported to `code_chunk`
/// (2-branch: vector + content BM25). Bench-local; production untouched.
async fn arm_rrf(
    db: &Surreal<Db>,
    emb: &Embedder,
    project: &str,
    q: &str,
    wide: usize,
    k: usize,
    rrf_k: u64,
) -> Vec<CodeSearchResult> {
    let Ok(qv) = emb.embed_single(q) else {
        return vec![];
    };
    let sql = format!(
        "search::rrf([\
             (SELECT id, vector::distance::knn() AS distance \
              FROM code_chunk WHERE embedding <|{wide},80|> $qv \
              AND (project=$p OR project='global')),\
             (SELECT id, search::score(1) AS ft_score \
              FROM code_chunk WHERE content @1@ $q \
              AND (project=$p OR project='global') \
              ORDER BY ft_score DESC LIMIT {wide})\
         ], {k}, {rrf_k})"
    );
    let fused: Vec<RrfResult> = match db
        .query(&sql)
        .bind(("qv", qv.to_vec()))
        .bind(("q", q.to_string()))
        .bind(("p", project.to_string()))
        .await
    {
        Ok(mut r) => r.take(0).unwrap_or_default(),
        Err(_) => vec![],
    };
    let ids: Vec<RecordId> = fused.iter().filter_map(|f| f.id.clone()).collect();
    if ids.is_empty() {
        return vec![];
    }
    // Fetch the fused chunks' fields, then re-emit in RRF order.
    let rows: Vec<CRow> = match db
        .query(
            "SELECT id, path, language, content, start_line, end_line \
             FROM code_chunk WHERE id IN $ids",
        )
        .bind(("ids", ids.clone()))
        .await
    {
        Ok(mut r) => r.take(0).unwrap_or_default(),
        Err(_) => vec![],
    };
    let mut by: HashMap<String, CRow> = rows
        .into_iter()
        .map(|r| (format!("{:?}", r.id), r))
        .collect();
    fused
        .iter()
        .filter_map(|f| {
            let id = f.id.clone()?;
            let r = by.remove(&format!("{:?}", Some(id)))?;
            Some(crow_to_result(r, f.rrf_score.unwrap_or(0.0), "a2"))
        })
        .collect()
}

/// Does an indexed chunk for the symbol even exist? (catches the
/// `splitter.rs:74` min_chars=250 drop / indexing gap.)
async fn oracle_chunk_exists(db: &Surreal<Db>, project: &str, p: &Probe) -> bool {
    #[derive(Debug, SurrealValue)]
    struct C {
        c: Option<i64>,
    }
    let sql = "SELECT count() AS c FROM code_chunk \
               WHERE (project = $p OR project = 'global') AND path = $path \
                 AND start_line <= $e AND end_line >= $s GROUP ALL";
    let mut r = match db
        .query(sql)
        .bind(("p", project.to_string()))
        .bind(("path", p.path.clone()))
        .bind(("s", p.doc_top))
        .bind(("e", p.end))
        .await
    {
        Ok(r) => r,
        Err(_) => return false,
    };
    let rows: Vec<C> = r.take(0).unwrap_or_default();
    rows.first().and_then(|x| x.c).unwrap_or(0) > 0
}

/// Arm grep — token-cost baseline only. Generous to grep (distinctive len≥4
/// terms → fewer files → lower grep cost → a conservative bar).
fn grep_tokens_and_filehit(q: &str, files: &[(String, String, usize)], p: &Probe) -> (bool, usize) {
    let mut seen = HashSet::new();
    let terms: Vec<String> = tokens(q)
        .into_iter()
        .filter(|t| t.len() >= 4)
        .filter(|t| seen.insert(t.clone()))
        .collect();
    if terms.is_empty() {
        return (false, 0);
    }
    let mut toks = 0usize;
    let mut hit = false;
    for (rel, content_lower, et) in files {
        if terms.iter().any(|t| content_lower.contains(t.as_str())) {
            toks += *et;
            if rel == &p.path {
                hit = true;
            }
        }
    }
    (hit, toks)
}

// ---------------------------------------------------------------------------

#[derive(Default)]
struct Tax {
    /// CORRECT: a returned chunk overlaps [doc_top..end] — the agent actually
    /// got the function. This is the headline correct set.
    chunk_hit: usize,
    /// Subset of `chunk_hit` the doc-excluding strict oracle would have WRONGLY
    /// missed (audit of the oracle correction; transparency, not flatter).
    strict_artifact: usize,
    /// file-hit but no chunk over [doc_top..end] (agent got the file, wrong
    /// region/function), and the right chunk DOES surface in WIDE → ranking/
    /// pool buried it. Recoverable.
    region_recoverable: usize,
    /// …and the right chunk does NOT surface even in WIDE → chunking/
    /// representation. Deep.
    region_deep: usize,
    /// file-miss, but file present in WIDE / BM25-wide → recoverable.
    file_recoverable: usize,
    /// file-miss, absent even in WIDE / BM25-wide → deep representation.
    file_deep: usize,
    /// no indexed chunk over [doc_top..end] at all → indexing gap
    /// (`splitter.rs:74` min_chars drop). Infrastructural.
    infra_unindexed: usize,
}

fn pct(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        100.0 * n as f64 / d as f64
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mut root = ".".to_string();
    let mut project = "coderetr".to_string();
    let mut export = "benchmark/data/code_retrieval_results.json".to_string();
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--root" => root = a.next().unwrap_or(root),
            "--project" => project = a.next().unwrap_or(project),
            "--export-json" => export = a.next().unwrap_or(export),
            "-h" | "--help" => {
                eprintln!(
                    "usage: code-retrieval-bench [--root <dir>] [--project <name>] \
                     [--export-json <path>]\ndefaults: --root . --project coderetr"
                );
                return Ok(());
            }
            other => eprintln!("warning: unknown arg '{other}' (ignored)"),
        }
    }

    println!("=== code-retrieval-bench: {project} ({root}) — correctness diagnosis ===");
    let db = db::connect_mem().await?;
    let embedder = Embedder::new()?;
    init_schema(&db, embedder.dimension()).await?;
    let root_path = std::path::Path::new(&root);

    let idx = index_repo(&db, &embedder, &project, root_path, &IndexOpts::default()).await?;
    println!(
        "index_repo: {} files, {} chunks, {} symbols",
        idx.indexed, idx.chunks, idx.symbols
    );
    if idx.symbols == 0 {
        std::process::exit(Verdict::Skip("0 code symbols indexed".into()).emit());
    }

    // Probes (mirror corpus_code_bench exactly).
    let mut sres = db
        .query(
            "SELECT name, qualified, kind, doc, path, start_line, end_line, signature \
             FROM code_symbol WHERE project = $p \
               AND kind IN ['function','method','struct','enum','trait']",
        )
        .bind(("p", project.clone()))
        .await?;
    let syms: Vec<SymRow> = sres.take(0).unwrap_or_default();
    let n_syms = syms.len();
    let mut probes: Vec<Probe> = Vec::new();
    for s in &syms {
        let (Some(path), Some(st), Some(en)) = (&s.path, s.start_line, s.end_line) else {
            continue;
        };
        let Some(doc) = s
            .doc
            .clone()
            .filter(|d| !d.trim().is_empty())
            .or_else(|| doc_above(&root, path, st))
        else {
            continue;
        };
        let sent = first_sentence(&doc);
        if sent.split_whitespace().count() < 3 || sent.len() < 12 {
            continue;
        }
        probes.push(Probe {
            raw: sent.clone(),
            paraphrase: paraphrase(&sent, s),
            path: path.clone(),
            start: st,
            end: en,
            doc_top: doc_top(&root, path, st),
        });
    }
    let para: Vec<&Probe> = probes.iter().filter(|p| p.paraphrase.is_some()).collect();
    println!(
        "documented symbols usable: {} (of {} fn/type); paraphrasable: {}",
        probes.len(),
        n_syms,
        para.len()
    );
    if probes.len() < MIN_DOCUMENTED {
        std::process::exit(
            Verdict::Skip(format!(
                "only {} usable documented symbols; need ≥{MIN_DOCUMENTED}",
                probes.len()
            ))
            .emit(),
        );
    }
    if para.len() < MIN_DOCUMENTED {
        std::process::exit(
            Verdict::Skip(format!(
                "only {} paraphrasable symbols; need ≥{MIN_DOCUMENTED}",
                para.len()
            ))
            .emit(),
        );
    }

    // Walk once: code files for the grep token baseline.
    let walked = walk(root_path, &WalkOpts::default())?;
    let mut files: Vec<(String, String, usize)> = Vec::with_capacity(walked.len());
    for f in &walked {
        let Ok(bytes) = std::fs::read(&f.abs) else {
            continue;
        };
        let Ok(src) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let et = est_tokens(src);
        files.push((f.rel.clone(), src.to_lowercase(), et));
    }

    // --- Paraphrase: headline = CORRECTED chunk-hit (agent got the function);
    // localize every non-hit; tokens informational on the correct set only. ---
    let n = para.len();
    let mut tax = Tax::default();
    let mut raw_file_hit = 0usize;
    let mut raw_chunk_hit = 0usize;
    // raw-doc chunk-hit per arm — the pre-registered easy-query regression
    // guard (gate criterion 3).
    let (mut a0_raw, mut a1_raw, mut a2_raw) = (0usize, 0usize, 0usize);
    let mut hit_cs_tok: Vec<usize> = Vec::new(); // tokens on the CORRECT set only
    let mut grep_tok: Vec<usize> = Vec::new();
    let mut grep_file_hit = 0usize;
    const RRF_K: u64 = 10; // hifz memory-search default (src/search.rs:50)

    for p in &probes {
        let rh = run_search(&db, &embedder, &project, &p.raw, K).await;
        if file_rank(&rh, p).is_some() {
            raw_file_hit += 1;
        }
        if chunk_rank(&rh, p).is_some() {
            raw_chunk_hit += 1;
            a0_raw += 1;
        }
        if chunk_rank(
            &arm_widen_max(&db, &embedder, &project, &p.raw, WIDE, K).await,
            p,
        )
        .is_some()
        {
            a1_raw += 1;
        }
        if chunk_rank(
            &arm_rrf(&db, &embedder, &project, &p.raw, WIDE, K, RRF_K).await,
            p,
        )
        .is_some()
        {
            a2_raw += 1;
        }
    }
    // A0's recoverable misses (indices into `para`) — the pre-registered
    // denominator for gate criterion 2 (no post-hoc redefinition).
    let mut recoverable_idx: Vec<usize> = Vec::new();
    for (pi, p) in para.iter().enumerate() {
        let q = p.paraphrase.as_deref().unwrap();
        let hits_k = run_search(&db, &embedder, &project, q, K).await;
        let f_hit = file_rank(&hits_k, p).is_some();
        let c_hit = chunk_rank(&hits_k, p).is_some(); // CORRECTED [doc_top..end]
        let c_hit_strict = chunk_rank_strict(&hits_k, p).is_some();

        let (g_hit, g_tok) = grep_tokens_and_filehit(q, &files, p);
        grep_tok.push(g_tok);
        if g_hit {
            grep_file_hit += 1;
        }

        if c_hit {
            tax.chunk_hit += 1;
            if !c_hit_strict {
                tax.strict_artifact += 1;
            }
            hit_cs_tok.push(hits_k.iter().map(|h| est_tokens(&h.snippet)).sum());
            continue;
        }
        // Not a correct retrieval — localize.
        if f_hit {
            // right FILE, wrong region/function — agent did NOT get it.
            let wide = run_search(&db, &embedder, &project, q, WIDE).await;
            if chunk_rank(&wide, p).is_some() {
                tax.region_recoverable += 1;
                recoverable_idx.push(pi);
            } else {
                tax.region_deep += 1;
            }
        } else if !oracle_chunk_exists(&db, &project, p).await {
            tax.infra_unindexed += 1;
        } else {
            let wide = run_search(&db, &embedder, &project, q, WIDE).await;
            let in_wide = file_rank(&wide, p).is_some();
            let in_bm25_wide = bm25_file_present(&db, &project, q, WIDE, p).await;
            if in_wide || in_bm25_wide {
                tax.file_recoverable += 1;
                recoverable_idx.push(pi);
            } else {
                tax.file_deep += 1;
            }
        }
    }

    let genuine_fail = n - tax.chunk_hit;
    let recoverable = tax.region_recoverable + tax.file_recoverable;
    let deep = tax.region_deep + tax.file_deep;
    let infra = tax.infra_unindexed;
    let chunk_hit_recall = tax.chunk_hit as f64 / n as f64;
    let file_hit = n - (tax.file_recoverable + tax.file_deep + tax.infra_unindexed);
    let mean = |v: &[usize]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<usize>() as f64 / v.len() as f64
        }
    };
    let cs_avg = mean(&hit_cs_tok);
    let g_avg = mean(&grep_tok);

    println!();
    println!("── PARAPHRASE (semantic test, n={n}) — RETRIEVAL CORRECTNESS ──");
    println!(
        "  HEADLINE  chunk-hit@{K} (agent actually got the function) = {} [{:.1}%]   \
         genuine_fail = {} [{:.1}%]",
        tax.chunk_hit,
        pct(tax.chunk_hit, n),
        genuine_fail,
        pct(genuine_fail, n),
    );
    println!(
        "  failure breakdown:  recoverable={} [{:.1}%]  deep={} [{:.1}%]  \
         infra(unindexed)={} [{:.1}%]",
        recoverable,
        pct(recoverable, n),
        deep,
        pct(deep, n),
        infra,
        pct(infra, n),
    );
    println!(
        "    ├─ right-file/wrong-region:  recoverable={}  deep={}",
        tax.region_recoverable, tax.region_deep
    );
    println!(
        "    └─ wrong-file:               recoverable={}  deep={}  unindexed={}",
        tax.file_recoverable, tax.file_deep, tax.infra_unindexed
    );
    println!(
        "  (file-hit@{K} = {} [{:.1}%] — looser, grep-comparable, NOT the headline)",
        file_hit,
        pct(file_hit, n),
    );
    println!(
        "  oracle-correction audit: {} [{:.1}%] of correct hits would have been \
         WRONGLY missed by the doc-excluding strict span (why [doc_top..end])",
        tax.strict_artifact,
        pct(tax.strict_artifact, n),
    );
    println!();
    println!("── raw-doc sanity (lexical-overlap biased, not gated) ──");
    println!(
        "  file-hit@{K} = {:.3}   chunk-hit@{K} = {:.3}   (n={})",
        raw_file_hit as f64 / probes.len() as f64,
        raw_chunk_hit as f64 / probes.len() as f64,
        probes.len()
    );
    println!();
    println!(
        "── tokens (INFORMATIONAL ONLY; null value on the {:.1}% it gets wrong; \
         not gated, not a ratio claim) ──",
        pct(genuine_fail, n)
    );
    println!(
        "  code_search avg_tok on the {:.1}% CORRECT set = {cs_avg:.0}   |   \
         grep avg_tok (file-cost baseline) = {g_avg:.0}   |   grep file-hit = {} [{:.1}%]",
        pct(tax.chunk_hit, n),
        grep_file_hit,
        pct(grep_file_hit, n),
    );

    // ── EXPERIMENT: A0 control vs A1 widen+max vs A2 widen+RRF.
    // Production search_code is NOT modified; A1/A2 are bench-local raw SQL. ──
    let (mut a0_hit, mut a1_hit, mut a2_hit) = (0usize, 0usize, 0usize);
    let (mut a0_ms, mut a1_ms, mut a2_ms): (Vec<f64>, Vec<f64>, Vec<f64>) =
        (vec![], vec![], vec![]);
    let (mut a0_tok, mut a1_tok, mut a2_tok): (Vec<usize>, Vec<usize>, Vec<usize>) =
        (vec![], vec![], vec![]);
    let (mut conv1, mut conv2) = (0usize, 0usize);
    for (pi, p) in para.iter().enumerate() {
        let q = p.paraphrase.as_deref().unwrap();
        let t = std::time::Instant::now();
        let h0 = run_search(&db, &embedder, &project, q, K).await;
        a0_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        let h1 = arm_widen_max(&db, &embedder, &project, q, WIDE, K).await;
        a1_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        let h2 = arm_rrf(&db, &embedder, &project, q, WIDE, K, RRF_K).await;
        a2_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        let (c0, c1, c2) = (
            chunk_rank(&h0, p).is_some(),
            chunk_rank(&h1, p).is_some(),
            chunk_rank(&h2, p).is_some(),
        );
        if c0 {
            a0_hit += 1;
            a0_tok.push(h0.iter().map(|h| est_tokens(&h.snippet)).sum());
        }
        if c1 {
            a1_hit += 1;
            a1_tok.push(h1.iter().map(|h| est_tokens(&h.snippet)).sum());
        }
        if c2 {
            a2_hit += 1;
            a2_tok.push(h2.iter().map(|h| est_tokens(&h.snippet)).sum());
        }
        if recoverable_idx.contains(&pi) {
            if c1 {
                conv1 += 1;
            }
            if c2 {
                conv2 += 1;
            }
        }
    }
    let pctf = |x: usize| 100.0 * x as f64 / n as f64;
    let meanf = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let meanu = |v: &[usize]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<usize>() as f64 / v.len() as f64
        }
    };
    let p90 = |v: &[f64]| {
        if v.is_empty() {
            return 0.0;
        }
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        s[((s.len() as f64 * 0.9) as usize).min(s.len() - 1)]
    };
    let nrec = recoverable_idx.len();
    println!();
    println!(
        "── EXPERIMENT: A0 control · A1 widen+max · A2 widen+RRF \
         (n={n}, WIDE={WIDE}, rrf_k={RRF_K}) ──"
    );
    let row = |a: &str, hit: usize, conv: usize, raw: usize, ms: &[f64], tok: &[usize]| {
        println!(
            "  {a:<3} chunk-hit {hit:>4} [{:>5.1}%]  Δ{:+5.1}pp  recov→hit {conv:>3}/{nrec:<3}  \
             raw {raw:>4}/{:<4}  avg {:>6.1}ms  p90 {:>6.1}ms  tok {:>6.0}",
            pctf(hit),
            pctf(hit) - pctf(a0_hit),
            probes.len(),
            meanf(ms),
            p90(ms),
            meanu(tok),
        );
    };
    row("A0", a0_hit, 0, a0_raw, &a0_ms, &a0_tok);
    row("A1", a1_hit, conv1, a1_raw, &a1_ms, &a1_tok);
    row("A2", a2_hit, conv2, a2_raw, &a2_ms, &a2_tok);

    // ── Pre-registered ADOPT/REJECT gate — thresholds fixed in the plan
    // BEFORE this run, evaluated mechanically (no post-hoc reinterpretation). ──
    // Tie-break: prefer A1 unless A2 leads by > 0.5pp (Occam — less change).
    let prefer_a1 = a1_hit >= a2_hit || (pctf(a2_hit) - pctf(a1_hit)) <= 0.5;
    let (best_name, best_hit, best_conv, best_raw, best_ms): (&str, usize, usize, usize, &[f64]) =
        if prefer_a1 {
            ("A1", a1_hit, conv1, a1_raw, &a1_ms)
        } else {
            ("A2", a2_hit, conv2, a2_raw, &a2_ms)
        };
    let d_delta = pctf(best_hit) - pctf(a0_hit);
    let best_p90 = p90(best_ms);
    let a0_p90 = p90(&a0_ms);
    let c1ok = d_delta >= 1.0;
    let c2ok = nrec == 0 || (best_conv as f64 / nrec as f64) >= 0.5;
    let c3ok = (best_raw as f64) >= (a0_raw as f64) - 0.005 * probes.len() as f64;
    let c4ok = best_p90 <= 2.5 * a0_p90.max(0.001);
    let adopt = c1ok && c2ok && c3ok && c4ok;
    let yn = |b: bool| if b { "PASS" } else { "FAIL" };
    println!();
    println!("── PRE-REGISTERED GATE (thresholds fixed in plan before run) ──");
    println!(
        "  C1 Δchunk-hit ≥ +1.0pp        : {d_delta:+.1}pp  {}",
        yn(c1ok)
    );
    println!(
        "  C2 ≥50% recoverable converted : {best_conv}/{nrec}  {}",
        yn(c2ok)
    );
    println!(
        "  C3 no raw-doc regression      : {best_name} raw {best_raw} vs A0 {a0_raw}  {}",
        yn(c3ok)
    );
    println!(
        "  C4 p90 ≤ 2.5× A0 p90          : {best_p90:.1}ms vs A0 {a0_p90:.1}ms  {}",
        yn(c4ok)
    );
    println!(
        "  EXPERIMENT VERDICT: {}",
        if adopt {
            format!(
                "ADOPT {best_name} — follow-up: port into src/code/search.rs (separate, re-verified)"
            )
        } else {
            "REJECT — null/insufficient; do NOT modify search_code".to_string()
        }
    );

    let json = serde_json::json!({
        "root": root, "project": project, "k": K, "wide": WIDE,
        "code_files": files.len(), "code_symbols": idx.symbols,
        "probes": probes.len(), "paraphrasable": n,
        "paraphrase": {
            "n": n,
            "chunk_hit": tax.chunk_hit, "chunk_hit_recall": chunk_hit_recall,
            "genuine_fail": genuine_fail, "genuine_fail_pct": pct(genuine_fail, n),
            "recoverable": recoverable, "deep": deep, "infra_unindexed": infra,
            "region_recoverable": tax.region_recoverable, "region_deep": tax.region_deep,
            "file_recoverable": tax.file_recoverable, "file_deep": tax.file_deep,
            "file_hit_looser": file_hit,
            "oracle_strict_artifact": tax.strict_artifact,
            "code_search_avg_tok_on_correct": cs_avg,
            "grep_avg_tok": g_avg, "grep_file_hit": grep_file_hit
        },
        "raw_sanity": { "n": probes.len(), "file_hit": raw_file_hit, "chunk_hit": raw_chunk_hit },
        "experiment": {
            "n": n, "wide": WIDE, "rrf_k": RRF_K, "recoverable_n": nrec,
            "a0": { "chunk_hit": a0_hit, "raw_chunk_hit": a0_raw,
                    "avg_ms": meanf(&a0_ms), "p90_ms": p90(&a0_ms), "avg_tok": meanu(&a0_tok) },
            "a1": { "chunk_hit": a1_hit, "raw_chunk_hit": a1_raw, "recov_converted": conv1,
                    "avg_ms": meanf(&a1_ms), "p90_ms": p90(&a1_ms), "avg_tok": meanu(&a1_tok) },
            "a2": { "chunk_hit": a2_hit, "raw_chunk_hit": a2_raw, "recov_converted": conv2,
                    "avg_ms": meanf(&a2_ms), "p90_ms": p90(&a2_ms), "avg_tok": meanu(&a2_tok) },
            "gate": { "best": best_name, "c1_delta_ge_1pp": c1ok, "c2_recov_ge_50pct": c2ok,
                      "c3_no_raw_regression": c3ok, "c4_p90_le_2_5x": c4ok, "adopt": adopt }
        }
    });
    let _ = std::fs::create_dir_all("benchmark/data");
    match std::fs::write(&export, serde_json::to_string_pretty(&json)?) {
        Ok(_) => println!("\nwrote {export}"),
        Err(e) => eprintln!("\nwarning: could not write {export}: {e}"),
    }

    // Gate: correctness only. PASS iff CORRECTED chunk-hit recall ≥ 0.50 (the
    // project's own pre-registered floor, corpus_code_bench.rs:372). The
    // looser file-hit is NOT gated.
    const FLOOR: f64 = 0.50;
    let verdict = if chunk_hit_recall >= FLOOR {
        Verdict::Pass
    } else {
        Verdict::Fail(vec![format!(
            "paraphrase chunk-hit recall {chunk_hit_recall:.3} below correctness \
             floor {FLOOR} — genuine_fail {:.1}% (recoverable {:.1}%, deep {:.1}%, \
             infra {:.1}%)",
            pct(genuine_fail, n),
            pct(recoverable, n),
            pct(deep, n),
            pct(infra, n),
        )])
    };
    std::process::exit(verdict.emit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `content` to a unique temp file; return (root, rel) for `doc_top`.
    fn fixture(content: &str) -> (String, String) {
        let dir = std::env::temp_dir().join(format!(
            "crb_doctop_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let rel = "f.rs";
        std::fs::write(dir.join(rel), content).unwrap();
        (dir.to_string_lossy().into_owned(), rel.to_string())
    }

    #[test]
    fn two_line_doc_block_returns_first_doc_line() {
        // L1 ///, L2 ///, L3 fn  → start_line=3 → doc_top=1
        let (root, rel) = fixture("/// summary\n/// more\nfn foo() {}\n");
        assert_eq!(doc_top(&root, &rel, 3), 1);
    }

    #[test]
    fn no_doc_returns_start_line() {
        // L1 code, L2 fn → start_line=2, no comment above → returns start_line
        let (root, rel) = fixture("fn bar() {}\nfn foo() {}\n");
        assert_eq!(doc_top(&root, &rel, 2), 2);
    }

    #[test]
    fn skips_attribute_and_blank_between_doc_and_def() {
        // L1 ///, L2 #[inline], L3 (blank), L4 fn → start_line=4 → doc_top=1
        let (root, rel) = fixture("/// d\n#[inline]\n\nfn foo() {}\n");
        assert_eq!(doc_top(&root, &rel, 4), 1);
    }

    #[test]
    fn start_line_lt_2_is_guarded() {
        let (root, rel) = fixture("/// d\nfn foo() {}\n");
        assert_eq!(doc_top(&root, &rel, 1), 1);
    }

    #[test]
    fn inner_doc_and_top_of_file_reaches_line_one() {
        // single //! at L1, fn at L2 → doc_top=1 (loop hits j==0)
        let (root, rel) = fixture("//! crate doc\nfn foo() {}\n");
        assert_eq!(doc_top(&root, &rel, 2), 1);
    }

    #[test]
    fn blank_then_code_above_is_not_a_doc() {
        // L1 code, L2 blank, L3 fn → no doc → returns start_line (3)
        let (root, rel) = fixture("fn a() {}\n\nfn foo() {}\n");
        assert_eq!(doc_top(&root, &rel, 3), 3);
    }

    #[test]
    fn missing_file_falls_back_to_start_line() {
        assert_eq!(doc_top("/no/such/dir", "x.rs", 7), 7);
    }

    // Corrected vs strict chunk overlap: a chunk that ends *before* the def
    // line but covers the doc must count as a hit under the corrected span and
    // a miss under the strict span (this is the oracle bug being fixed).
    fn res(path: &str, sl: usize, el: usize) -> hifz::code::search::CodeSearchResult {
        hifz::code::search::CodeSearchResult {
            id: String::new(),
            file_id: String::new(),
            path: path.to_string(),
            language: "rust".into(),
            start_line: sl,
            end_line: el,
            snippet: String::new(),
            score: 1.0,
            via: "test",
        }
    }

    #[test]
    fn corrected_span_counts_doc_only_chunk_strict_does_not() {
        let p = Probe {
            raw: String::new(),
            paraphrase: None,
            path: "src/x.rs".into(),
            start: 50,
            end: 80,
            doc_top: 45,
        };
        // chunk covers lines 44..49 — the doc block, ends before def line 50.
        let hits = vec![res("src/x.rs", 44, 49)];
        assert_eq!(chunk_rank(&hits, &p), Some(0)); // corrected: doc_top=45 ≤ 49
        assert_eq!(chunk_rank_strict(&hits, &p), None); // strict: 50 > 49
        assert_eq!(file_rank(&hits, &p), Some(0));
        // wrong file is never a hit (exact path, no basename fallback)
        let other = vec![res("crate/x.rs", 44, 90)];
        assert_eq!(file_rank(&other, &p), None);
        assert_eq!(chunk_rank(&other, &p), None);
    }

    // Smoke: the A1/A2 raw SurrealQL must PARSE and return rows on a tiny
    // in-mem corpus — i.e. don't ship unparsed SurrealQL (same discipline as
    // the doc_top tests). The helpers swallow query errors into `vec![]`, so a
    // malformed query manifests as an empty result on a corpus where
    // `search_code` itself returns hits — the assertion below catches that.
    async fn tiny_corpus() -> (Surreal<Db>, Embedder, String) {
        let db = db::connect_mem().await.unwrap();
        let emb = Embedder::new().unwrap();
        init_schema(&db, emb.dimension()).await.unwrap();
        let dir = std::env::temp_dir().join(format!(
            "crb_arms_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "/// Parses a configuration file from disk and returns settings.\n\
             pub fn load_configuration(path: &str) -> u32 { path.len() as u32 }\n\n\
             /// Computes the arithmetic mean of a slice of integers.\n\
             pub fn average(xs: &[i64]) -> i64 { xs.iter().sum::<i64>() / xs.len() as i64 }\n",
        )
        .unwrap();
        let rep = index_repo(
            &db,
            &emb,
            "t",
            std::path::Path::new(&dir),
            &IndexOpts::default(),
        )
        .await
        .unwrap();
        assert!(rep.symbols > 0, "fixture must index ≥1 symbol");
        (db, emb, "t".to_string())
    }

    #[tokio::test]
    async fn arm_widen_max_sql_parses_and_returns_rows() {
        let (db, emb, proj) = tiny_corpus().await;
        // sanity: production search_code finds something on this corpus…
        let base = run_search(&db, &emb, &proj, "configuration file", 10).await;
        assert!(
            !base.is_empty(),
            "search_code returned nothing — corpus bug"
        );
        // …so an empty A1 here means the raw widen+max SurrealQL failed to parse.
        let r = arm_widen_max(&db, &emb, &proj, "configuration file", 50, 10).await;
        assert!(!r.is_empty(), "A1 widen+max SurrealQL parsed to zero rows");
    }

    #[tokio::test]
    async fn arm_rrf_sql_parses_and_returns_rows() {
        let (db, emb, proj) = tiny_corpus().await;
        let base = run_search(&db, &emb, &proj, "arithmetic mean", 10).await;
        assert!(
            !base.is_empty(),
            "search_code returned nothing — corpus bug"
        );
        // Empty here means the `search::rrf([...])` SurrealQL failed to parse.
        let r = arm_rrf(&db, &emb, &proj, "arithmetic mean", 50, 10, 10).await;
        assert!(!r.is_empty(), "A2 widen+RRF SurrealQL parsed to zero rows");
    }
}
