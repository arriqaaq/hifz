//! Code-retrieval token-efficiency: **atlas vs code-search vs grep**, on a real
//! tree, scored by the corpus_code_bench docstring→code oracle plus a
//! token-cost dimension.
//!
//! This is deliberately NOT a cross-tool contest (a Burhan debate killed the
//! hifz-vs-Fullerenes head-to-head: different-shaped artifacts → category
//! error). Here every arm answers the SAME questions against the SAME oracle
//! with the SAME `chars/4` estimator, and the anchor is the **grep baseline** —
//! the universal fallback graphify/Fullerenes themselves benchmark against,
//! which is exactly why the reduction ratio is legitimately comparable to
//! their published numbers. atlas-vs-code-search is an internal A/B reported
//! with one identical metric; the gate makes **no** winner claim about it.
//!
//! Ground truth = docstring→code (CodeSearchNet style; copied verbatim from
//! `benchmark/corpus_code_bench.rs`, which is left untouched — this bench is
//! additive: same oracle + a token-cost dimension + atlas & grep arms it
//! lacks). raw-doc style = first doc sentence verbatim (lexical-overlap bias);
//! paraphrase style = identifiers deterministically stripped (the real
//! semantic test). The gate lives in the paraphrase style.
//!
//! Usage:
//!   cargo run --release --features atlas --bin code-retrieval-bench
//!   cargo run --release --features atlas --bin code-retrieval-bench -- \
//!       --root . --limit 10 --project coderetr

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use atlas::query::query as atlas_query;
use atlas::store::{Store, init_atlas_schema};
use hifz::code::index::{IndexOpts, index_repo};
use hifz::code::search::{CodeSearchOpts, search_code};
use hifz::db::{self, Db, init_schema};
use hifz::embed::Embedder;
use hifz::rid_to_string;
use kernel::code_parse::walker::{WalkOpts, walk};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

#[path = "corpus_common.rs"]
mod corpus_common;
use corpus_common::{Verdict, mrr, recall_at_k};

fn est_tokens(s: &str) -> usize {
    (s.len() / 4).max(1)
}

const MIN_DOCUMENTED: usize = 25;

// ---------------------------------------------------------------------------
// Oracle helpers — copied verbatim from benchmark/corpus_code_bench.rs
// (that file is intentionally NOT modified). Probe extended with `qualified`
// for the atlas-id oracle map.
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
/// immediately above the definition, skipping interleaved attribute/blank
/// lines (`code_symbol.doc` is reserved but never populated by codeintel).
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
    qualified: Option<String>,
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
    if kept.len() < 3 {
        return None;
    }
    Some(kept.join(" "))
}

/// Chunk-hit ↔ probe match: same path (exact, else basename) + line overlap.
fn overlaps(hp: &str, hs: i64, he: i64, p: &Probe) -> bool {
    let same = hp == p.path || hp.rsplit('/').next() == p.path.rsplit('/').next();
    same && hs <= p.end && p.start <= he
}

// ---------------------------------------------------------------------------
// Arms
// ---------------------------------------------------------------------------

/// Arm B — `search_code` (the engine the `hifz_code_search` MCP tool wraps).
/// Returns (oracle rank, tokens the agent would read = Σ snippet tokens).
async fn arm_code_search(
    db: &Surreal<Db>,
    emb: &Embedder,
    project: &str,
    q: &str,
    limit: usize,
    p: &Probe,
) -> (Option<usize>, usize) {
    let opts = CodeSearchOpts {
        limit,
        project: Some(project.to_string()),
        ..Default::default()
    };
    match search_code(db, emb, q, &opts).await {
        Ok(hits) => {
            let rank = hits
                .iter()
                .position(|h| overlaps(&h.path, h.start_line as i64, h.end_line as i64, p));
            let toks = hits.iter().map(|h| est_tokens(&h.snippet)).sum();
            (rank, toks)
        }
        Err(_) => (None, 0),
    }
}

/// Arm A — atlas graph query. Oracle = the atlas_node id for the probe's
/// qualified symbol (pre-resolved map). Honestly weak on NL docstrings:
/// `query()` is BM25 on the symbol *name*; the stored embedding is unused by
/// the query path. A low number here is a real finding, not a defect.
async fn arm_atlas(
    store: &Store,
    q: &str,
    limit: usize,
    oracle_id: Option<&String>,
) -> (Option<usize>, usize) {
    match atlas_query(store, q, limit).await {
        Ok(hits) => {
            let toks = hits
                .iter()
                .map(|h| est_tokens(h.snippet.as_deref().unwrap_or("")))
                .sum();
            let rank = match oracle_id {
                Some(id) => hits.iter().position(|h| &h.id == id),
                None => None,
            };
            (rank, toks)
        }
        Err(_) => (None, 0),
    }
}

/// Arm C — realistic grep. Deliberately *generous to grep*: only distinctive
/// terms (len ≥ 4 — drops most English stopwords), so fewer files match →
/// lower grep token cost → a harder, more conservative bar for hifz's claim.
/// `grep -ri term1|term2|…`; recall = is the oracle file in the matched set?
/// (grep finds the file or it doesn't — no intra-file ranking; generous).
fn arm_grep(
    q: &str,
    files: &[(String, String, usize)], // (rel, content_lower, est_tokens)
    p: &Probe,
) -> (Option<usize>, usize) {
    let terms: Vec<String> = {
        let mut seen = HashSet::new();
        tokens(q)
            .into_iter()
            .filter(|t| t.len() >= 4)
            .filter(|t| seen.insert(t.clone()))
            .collect()
    };
    if terms.is_empty() {
        return (None, 0);
    }
    let mut toks = 0usize;
    let mut hit = false;
    for (rel, content_lower, et) in files {
        if terms.iter().any(|t| content_lower.contains(t.as_str())) {
            toks += *et;
            let same = rel == &p.path || rel.rsplit('/').next() == p.path.rsplit('/').next();
            if same {
                hit = true;
            }
        }
    }
    (if hit { Some(0) } else { None }, toks)
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

struct Row {
    style: &'static str,
    arm: &'static str,
    r1: f64,
    r5: f64,
    r10: f64,
    mrr: f64,
    avg_tok: f64,
    x_grep: f64,
    x_corpus: f64,
    n: usize,
}

fn agg(
    style: &'static str,
    arm: &'static str,
    ranks: &[Option<usize>],
    toks: &[usize],
    grep_avg: f64,
    corpus_tok: usize,
) -> Row {
    let avg_tok = if toks.is_empty() {
        0.0
    } else {
        toks.iter().sum::<usize>() as f64 / toks.len() as f64
    };
    let safe = avg_tok.max(1.0);
    Row {
        style,
        arm,
        r1: recall_at_k(ranks, 1),
        r5: recall_at_k(ranks, 5),
        r10: recall_at_k(ranks, 10),
        mrr: mrr(ranks),
        avg_tok,
        x_grep: grep_avg / safe,
        x_corpus: corpus_tok as f64 / safe,
        n: ranks.len(),
    }
}

// ---------------------------------------------------------------------------

#[derive(Debug, SurrealValue)]
struct AtlasIdRow {
    qualified: Option<String>,
    id: Option<RecordId>,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mut root = ".".to_string();
    let mut project = "coderetr".to_string();
    let mut limit: usize = 10;
    let mut export = "benchmark/data/code_retrieval_results.json".to_string();
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--root" => root = a.next().unwrap_or(root),
            "--project" => project = a.next().unwrap_or(project),
            "--limit" => limit = a.next().and_then(|v| v.parse().ok()).unwrap_or(limit),
            "--export-json" => export = a.next().unwrap_or(export),
            "-h" | "--help" => {
                eprintln!(
                    "usage: code-retrieval-bench [--root <dir>] [--limit N] \
                     [--project <name>] [--export-json <path>]\n\
                     defaults: --root . --limit 10 --project coderetr"
                );
                return Ok(());
            }
            other => eprintln!("warning: unknown arg '{other}' (ignored)"),
        }
    }

    println!("=== code-retrieval-bench: {project} ({root}) ===");
    let db = db::connect_mem().await?;
    let embedder = Embedder::new()?;
    let dim = embedder.dimension();
    init_schema(&db, dim).await?;
    init_atlas_schema(&db, dim).await?;
    let store = Store::new(db.clone(), project.clone());
    let root_path = std::path::Path::new(&root);

    // --- Arm B ingest: hifz code index ---
    let idx = index_repo(&db, &embedder, &project, root_path, &IndexOpts::default()).await?;
    println!(
        "index_repo: {} files, {} chunks, {} symbols",
        idx.indexed, idx.chunks, idx.symbols
    );
    if idx.symbols == 0 {
        std::process::exit(
            Verdict::Skip("0 code symbols indexed (is --root a code dir?)".into()).emit(),
        );
    }

    // --- Arm A ingest: atlas code projection (independent of index_repo) ---
    let atlas_ok = match atlas::code::project_code_graph(&store, &embedder, root_path).await {
        Ok(acg) => {
            println!(
                "atlas project_code_graph: {} symbols, {} files, {} edges",
                acg.symbols, acg.files, acg.edges
            );
            acg.symbols > 0
        }
        Err(e) => {
            println!("atlas project_code_graph FAILED ({e}) — atlas arm → SKIP");
            false
        }
    };

    // --- Probes (mirror corpus_code_bench exactly) ---
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
            qualified: s.qualified.clone(),
        });
    }
    let paraphrasable = probes.iter().filter(|p| p.paraphrase.is_some()).count();
    println!(
        "documented symbols usable: {} (of {} fn/type symbols); paraphrasable: {}",
        probes.len(),
        n_syms,
        paraphrasable
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

    // --- Atlas oracle map: qualified → atlas_node id (Hit.id format) ---
    let atlas_map: HashMap<String, String> = if atlas_ok {
        let mut r = db
            .query(
                "SELECT qualified, id FROM atlas_node \
                 WHERE project = $p AND kind = 'code_symbol'",
            )
            .bind(("p", project.clone()))
            .await?;
        let rows: Vec<AtlasIdRow> = r.take(0).unwrap_or_default();
        rows.into_iter()
            .filter_map(|row| Some((row.qualified?, rid_to_string(&row.id?))))
            .collect()
    } else {
        HashMap::new()
    };

    // --- Walk once: code files for the grep arm + whole-corpus ceiling ---
    let walked = walk(root_path, &WalkOpts::default())?;
    let mut files: Vec<(String, String, usize)> = Vec::with_capacity(walked.len());
    let mut corpus_tok = 0usize;
    for f in &walked {
        let Ok(bytes) = std::fs::read(&f.abs) else {
            continue;
        };
        let Ok(src) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let et = est_tokens(src);
        corpus_tok += et;
        files.push((f.rel.clone(), src.to_lowercase(), et));
    }
    println!(
        "walked {} code files; whole-corpus ceiling = {} tokens (limit={})",
        files.len(),
        corpus_tok,
        limit
    );

    // --- Run all arms, both styles ---
    let mut g_raw_r = Vec::new();
    let mut g_raw_t = Vec::new();
    let mut c_raw_r = Vec::new();
    let mut c_raw_t = Vec::new();
    let mut a_raw_r = Vec::new();
    let mut a_raw_t = Vec::new();
    let mut g_par_r = Vec::new();
    let mut g_par_t = Vec::new();
    let mut c_par_r = Vec::new();
    let mut c_par_t = Vec::new();
    let mut a_par_r = Vec::new();
    let mut a_par_t = Vec::new();

    for p in &probes {
        // raw-doc style
        let (gr, gt) = arm_grep(&p.raw, &files, p);
        g_raw_r.push(gr);
        g_raw_t.push(gt);
        let (cr, ct) = arm_code_search(&db, &embedder, &project, &p.raw, limit, p).await;
        c_raw_r.push(cr);
        c_raw_t.push(ct);
        if atlas_ok {
            let oid = p.qualified.as_ref().and_then(|q| atlas_map.get(q));
            let (ar, at) = arm_atlas(&store, &p.raw, limit, oid).await;
            a_raw_r.push(ar);
            a_raw_t.push(at);
        }
        // paraphrase style
        if let Some(pq) = &p.paraphrase {
            let (gr, gt) = arm_grep(pq, &files, p);
            g_par_r.push(gr);
            g_par_t.push(gt);
            let (cr, ct) = arm_code_search(&db, &embedder, &project, pq, limit, p).await;
            c_par_r.push(cr);
            c_par_t.push(ct);
            if atlas_ok {
                let oid = p.qualified.as_ref().and_then(|q| atlas_map.get(q));
                let (ar, at) = arm_atlas(&store, pq, limit, oid).await;
                a_par_r.push(ar);
                a_par_t.push(at);
            }
        }
    }

    let mean = |v: &[usize]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<usize>() as f64 / v.len() as f64
        }
    };
    let g_raw_avg = mean(&g_raw_t);
    let g_par_avg = mean(&g_par_t);

    let mut rows: Vec<Row> = vec![
        agg("raw", "grep", &g_raw_r, &g_raw_t, g_raw_avg, corpus_tok),
        agg(
            "raw",
            "code_search",
            &c_raw_r,
            &c_raw_t,
            g_raw_avg,
            corpus_tok,
        ),
    ];
    if atlas_ok {
        rows.push(agg(
            "raw", "atlas", &a_raw_r, &a_raw_t, g_raw_avg, corpus_tok,
        ));
    }
    rows.push(agg(
        "paraphrase",
        "grep",
        &g_par_r,
        &g_par_t,
        g_par_avg,
        corpus_tok,
    ));
    rows.push(agg(
        "paraphrase",
        "code_search",
        &c_par_r,
        &c_par_t,
        g_par_avg,
        corpus_tok,
    ));
    if atlas_ok {
        rows.push(agg(
            "paraphrase",
            "atlas",
            &a_par_r,
            &a_par_t,
            g_par_avg,
            corpus_tok,
        ));
    }

    println!();
    println!(
        "{:<11} {:<12} {:>6} {:>6} {:>6} {:>6} {:>10} {:>8} {:>9} {:>5}",
        "style", "arm", "R@1", "R@5", "R@10", "MRR", "avg_tok", "x_grep", "x_corpus", "n"
    );
    for r in &rows {
        println!(
            "{:<11} {:<12} {:>6.3} {:>6.3} {:>6.3} {:>6.3} {:>10.0} {:>8.1} {:>9.1} {:>5}",
            r.style, r.arm, r.r1, r.r5, r.r10, r.mrr, r.avg_tok, r.x_grep, r.x_corpus, r.n
        );
    }
    if !atlas_ok {
        println!("atlas: SKIP (project_code_graph produced 0 symbols or failed)");
    }
    println!(
        "note: paraphrase is the controlled semantic test (raw has \
         lexical-overlap bias). grep = TOKEN-COST baseline only; its \"recall\" \
         is file-presence (different granularity), reported not gated. atlas \
         queries the symbol *name* (BM25) — NL docstrings ~never match a short \
         name, so atlas≈0 by construction (a true negative, not a harness bug)."
    );
    println!();

    // --- JSON artifact ---
    let json = serde_json::json!({
        "root": root, "project": project, "limit": limit,
        "code_files": files.len(), "code_symbols": idx.symbols,
        "atlas_ok": atlas_ok, "probes": probes.len(),
        "paraphrasable": paraphrasable, "whole_corpus_tokens": corpus_tok,
        "rows": rows.iter().map(|r| serde_json::json!({
            "style": r.style, "arm": r.arm, "r1": r.r1, "r5": r.r5,
            "r10": r.r10, "mrr": r.mrr, "avg_tok": r.avg_tok,
            "x_grep": r.x_grep, "x_corpus": r.x_corpus, "n": r.n
        })).collect::<Vec<_>>(),
    });
    let _ = std::fs::create_dir_all("benchmark/data");
    if let Err(e) = std::fs::write(&export, serde_json::to_string_pretty(&json)?) {
        eprintln!("warning: could not write {export}: {e}");
    } else {
        println!("wrote {export}");
    }

    // --- Gate: paraphrase (semantic) style. atlas never gated.
    //
    // grep is the TOKEN-COST baseline only (its legitimate role — exactly how
    // graphify/Fullerenes use it). grep has no intra-result ranking, so its
    // "recall" is mere file-presence; comparing that to code_search's
    // chunk-rank recall is a granularity category-error, and since every query
    // is derived from its own symbol's docstring, generous file-grep finds the
    // file ~always → such a condition is unconditionally unsatisfiable and
    // would not be a hifz signal. So the gate is: (1) code_search clears an
    // absolute semantic-recall floor, and (2) code_search is materially
    // cheaper than grep (the actual value proposition). The apples-to-apples
    // semantic lift vs BM25-only already lives in `corpus-code-bench`. ---
    const FLOOR_R5: f64 = 0.50;
    let cp = rows
        .iter()
        .find(|r| r.style == "paraphrase" && r.arm == "code_search")
        .expect("paraphrase/code_search row");
    let gp = rows
        .iter()
        .find(|r| r.style == "paraphrase" && r.arm == "grep")
        .expect("paraphrase/grep row");
    if cp.n < MIN_DOCUMENTED {
        std::process::exit(
            Verdict::Skip(format!(
                "only {} paraphrasable symbols; need ≥{MIN_DOCUMENTED}",
                cp.n
            ))
            .emit(),
        );
    }
    let mut reasons = Vec::new();
    if cp.r5 < FLOOR_R5 {
        reasons.push(format!(
            "code_search paraphrase Recall@5 {:.3} below floor {FLOOR_R5}",
            cp.r5
        ));
    }
    if cp.avg_tok > gp.avg_tok {
        reasons.push(format!(
            "code_search not cheaper than grep (avg_tok {:.0} > grep {:.0})",
            cp.avg_tok, gp.avg_tok
        ));
    }
    let verdict = if reasons.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail(reasons)
    };
    std::process::exit(verdict.emit())
}
