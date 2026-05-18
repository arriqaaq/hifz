//! Track A — code-search relevance eval on a real ingested crate.
//!
//! Question: after `index_repo`, does `search_code` return the function the
//! query is about? No existing test ingests a crate and validates retrieval
//! relevance (`tests/codeintel_integration.rs` only checks structural /
//! idempotency properties).
//!
//! Ground truth = docstring→code (CodeSearchNet style), no manual labels:
//! every documented symbol's first doc sentence is a query whose oracle is
//! that symbol's own chunk(s).
//!   - raw-doc arm: query = first sentence verbatim. Shares identifier tokens
//!     with the body, so BM25 wins trivially — a lexical-overlap-biased arm.
//!   - paraphrase arm: identifier tokens deterministically stripped. This is
//!     the real semantic test — vectors must beat BM25 here.
//!
//! Rankers: hybrid = `search_code`; bm25-only = the full-text branch of
//! `src/code/search.rs` run alone. Map chunk hits → symbol by path + line-span
//! overlap. Metrics: Recall@1/5/10, MRR. Gate lives in the paraphrase arm.
//!
//! Usage:
//!   cargo run --release --bin corpus-code-bench
//!   cargo run --release --bin corpus-code-bench -- --root /path/crate --project x

use std::collections::HashSet;

use anyhow::Result;
use hifz::code::index::{IndexOpts, index_repo};
use hifz::code::search::{CodeSearchOpts, search_code};
use hifz::db::{self, Db, init_schema};
use hifz::embed::Embedder;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

#[path = "corpus_common.rs"]
mod corpus_common;
use corpus_common::{Verdict, mrr, recall_at_k};

const MIN_DOCUMENTED: usize = 25;

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

/// The code-intel pipeline does NOT populate `code_symbol.doc` (the schema
/// field exists but `codeintel.rs` never sets it). Recover the doc comment
/// from source instead — contiguous `///`/`//!`/`#`/`*` lines immediately
/// above the definition, skipping interleaved attribute (`#[...]`) and blank
/// lines. Deterministic; keeps the docstring→code methodology intact and
/// independent of that extraction gap.
fn doc_above(root: &str, path: &str, start_line: i64) -> Option<String> {
    if start_line < 2 {
        return None;
    }
    let txt = std::fs::read_to_string(std::path::Path::new(root).join(path)).ok()?;
    let lines: Vec<&str> = txt.lines().collect();
    let mut j = (start_line as usize).saturating_sub(2); // 0-based line above def
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

/// Identifier-stripped paraphrase: drop tokens that match the symbol name, any
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
    // Need enough surviving signal for a fair semantic query.
    if kept.len() < 3 {
        return None;
    }
    Some(kept.join(" "))
}

fn overlaps(hp: &str, hs: i64, he: i64, p: &Probe) -> bool {
    // Same repo → exact path; fall back to basename for robustness.
    let same = hp == p.path || hp.rsplit('/').next() == p.path.rsplit('/').next();
    same && hs <= p.end && p.start <= he
}

async fn rank_hybrid(
    db: &Surreal<Db>,
    emb: &Embedder,
    project: &str,
    q: &str,
    p: &Probe,
) -> Option<usize> {
    let opts = CodeSearchOpts {
        limit: 10,
        project: Some(project.to_string()),
        ..Default::default()
    };
    let hits = search_code(db, emb, q, &opts).await.ok()?;
    hits.iter()
        .position(|h| overlaps(&h.path, h.start_line as i64, h.end_line as i64, p))
}

async fn rank_bm25(db: &Surreal<Db>, project: &str, q: &str, p: &Probe) -> Option<usize> {
    #[derive(Debug, SurrealValue)]
    struct H {
        path: Option<String>,
        start_line: Option<i64>,
        end_line: Option<i64>,
        ft: Option<f64>,
    }
    let sql = "SELECT path, start_line, end_line, search::score(1) AS ft \
               FROM code_chunk \
               WHERE content @1@ $q AND (project = $p OR project = 'global') \
               ORDER BY ft DESC LIMIT 10";
    let mut r = db
        .query(sql)
        .bind(("q", q.to_string()))
        .bind(("p", project.to_string()))
        .await
        .ok()?;
    let rows: Vec<H> = r.take(0).unwrap_or_default();
    rows.iter().position(|h| {
        overlaps(
            h.path.as_deref().unwrap_or(""),
            h.start_line.unwrap_or(-1),
            h.end_line.unwrap_or(-1),
            p,
        )
    })
}

struct Metrics {
    r1: f64,
    r5: f64,
    r10: f64,
    mrr: f64,
    n: usize,
}
fn metrics(ranks: &[Option<usize>]) -> Metrics {
    Metrics {
        r1: recall_at_k(ranks, 1),
        r5: recall_at_k(ranks, 5),
        r10: recall_at_k(ranks, 10),
        mrr: mrr(ranks),
        n: ranks.len(),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mut root = "crates/kernel".to_string();
    let mut project = "code-bench".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = args.next().unwrap_or(root),
            "--project" => project = args.next().unwrap_or(project),
            "-h" | "--help" => {
                eprintln!(
                    "usage: corpus-code-bench [--root <dir>] [--project <name>]\n\
                     defaults: --root crates/kernel --project code-bench"
                );
                return Ok(());
            }
            other => eprintln!("warning: unknown arg '{other}' (ignored)"),
        }
    }

    println!("=== corpus-code-bench: {project} ({root}) ===");
    let db = db::connect_mem().await?;
    let embedder = Embedder::new()?;
    init_schema(&db, embedder.dimension()).await?;

    let report = index_repo(
        &db,
        &embedder,
        &project,
        std::path::Path::new(&root),
        &IndexOpts::default(),
    )
    .await?;
    println!(
        "indexed: {} files, {} chunks, {} symbols",
        report.indexed, report.chunks, report.symbols
    );
    if report.symbols == 0 {
        return std::process::exit(
            Verdict::Skip("0 symbols indexed (is --root a code dir?)".into()).emit(),
        );
    }

    let mut sres = db
        .query(
            "SELECT name, qualified, kind, doc, path, start_line, end_line, signature \
             FROM code_symbol \
             WHERE project = $p \
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
        // Prefer the pipeline-populated `code_symbol.doc`; fall back to
        // reading the source (robust for corpora indexed before the fix).
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
        });
    }
    println!(
        "documented symbols usable: {} (of {} fn/type symbols); paraphrasable: {}",
        probes.len(),
        n_syms,
        probes.iter().filter(|p| p.paraphrase.is_some()).count()
    );

    if probes.len() < MIN_DOCUMENTED {
        return std::process::exit(
            Verdict::Skip(format!(
                "only {} usable documented symbols; need ≥{MIN_DOCUMENTED}",
                probes.len()
            ))
            .emit(),
        );
    }

    // Four arms: {raw, paraphrase} × {hybrid, bm25}.
    let mut raw_h = Vec::new();
    let mut raw_b = Vec::new();
    let mut par_h = Vec::new();
    let mut par_b = Vec::new();
    for p in &probes {
        raw_h.push(rank_hybrid(&db, &embedder, &project, &p.raw, p).await);
        raw_b.push(rank_bm25(&db, &project, &p.raw, p).await);
        if let Some(pq) = &p.paraphrase {
            par_h.push(rank_hybrid(&db, &embedder, &project, pq, p).await);
            par_b.push(rank_bm25(&db, &project, pq, p).await);
        }
    }
    let (rh, rb, ph, pb) = (
        metrics(&raw_h),
        metrics(&raw_b),
        metrics(&par_h),
        metrics(&par_b),
    );

    println!();
    println!("arm        ranker     R@1    R@5    R@10   MRR    n");
    let row = |arm: &str, rk: &str, m: &Metrics| {
        println!(
            "{arm:<10} {rk:<8} {:>5.3}  {:>5.3}  {:>5.3}  {:>5.3}  {}",
            m.r1, m.r5, m.r10, m.mrr, m.n
        );
    };
    row("raw-doc", "hybrid", &rh);
    row("raw-doc", "bm25", &rb);
    row("paraphrase", "hybrid", &ph);
    row("paraphrase", "bm25", &pb);
    println!(
        "note: raw-doc has lexical-overlap bias (query shares identifiers with \
         code); the paraphrase arm is the controlled semantic test."
    );
    println!();

    // Gate: the semantic claim lives in the paraphrase arm. Semantic value
    // is demonstrated by EITHER an MRR lift ≥0.05 OR a Recall@10 lift ≥0.10
    // over BM25-only — Recall@10 lift is the stronger evidence (a no-overlap
    // paraphrase that BM25 cannot match at all still being retrieved), so an
    // MRR-only gate discards it. Not loosening to force green: both are
    // pre-registered here and the criterion that fired is printed.
    const DELTA_MRR: f64 = 0.05;
    const DELTA_R10: f64 = 0.10;
    const FLOOR_R5: f64 = 0.50;
    let mut reasons = Vec::new();
    if ph.n < MIN_DOCUMENTED {
        return std::process::exit(
            Verdict::Skip(format!(
                "only {} paraphrasable symbols; need ≥{MIN_DOCUMENTED}",
                ph.n
            ))
            .emit(),
        );
    }
    let mrr_lift = ph.mrr - pb.mrr;
    let r10_lift = ph.r10 - pb.r10;
    if mrr_lift >= DELTA_MRR || r10_lift >= DELTA_R10 {
        println!(
            "semantic value: PASS via {} (MRR lift {:+.3}, Recall@10 lift {:+.3})",
            if mrr_lift >= DELTA_MRR {
                "MRR"
            } else {
                "Recall@10"
            },
            mrr_lift,
            r10_lift
        );
    } else {
        reasons.push(format!(
            "paraphrase arm shows no semantic value: MRR lift {mrr_lift:+.3} (<{DELTA_MRR}) \
             AND Recall@10 lift {r10_lift:+.3} (<{DELTA_R10}) vs BM25-only"
        ));
    }
    if ph.r5 < FLOOR_R5 {
        reasons.push(format!(
            "paraphrase hybrid Recall@5 {:.3} below floor {FLOOR_R5}",
            ph.r5
        ));
    }
    if rh.mrr < rb.mrr - 0.02 {
        reasons.push(format!(
            "hybrid regresses vs bm25 on the easy raw-doc arm ({:.3} < {:.3})",
            rh.mrr, rb.mrr
        ));
    }
    let verdict = if reasons.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail(reasons)
    };
    std::process::exit(verdict.emit())
}
