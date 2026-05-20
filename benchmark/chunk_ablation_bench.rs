//! Chunk-knob ablation: is `OVERLAP=150` / `MIN=250` justified by data?
//!
//! Sweeps overlap ∈ {0,150,300} × min ∈ {0,250} (6 cells) on a real tree and
//! measures, per cell:
//!   • symbol-body **coverage** — for every documented symbol, does some chunk
//!     FULLY cover its `[doc_top..end]` span (the agent gets the whole thing),
//!     only PARTIALLY overlap (fragmented), or NONE (lost)? This is pure
//!     (real `CodeSplitter`, no DB/embeddings) → deterministic & fully faithful.
//!   • **index size** — chunk count + total content bytes.
//!   • **retrieval** — corrected chunk-hit@10 of real `search_code` after a
//!     faithful per-cell re-index (mirrors src/code/index.rs Step 5+7 CREATE
//!     verbatim; production indexing is NOT modified).
//!
//! Faithfulness gate: the (overlap=150,min=250) cell must reproduce the
//! canonical ~97–98% chunk-hit (it IS the production default); if it doesn't,
//! the re-index harness is unfaithful → retrieval numbers are marked UNTRUSTED
//! (coverage/size are independent and remain trustworthy).
//!
//! Bench-local; no production code changed. Oracle helpers copied verbatim
//! from benchmark/code_retrieval_bench.rs (unit-tested there).
//!
//! Usage: cargo run --release --bin chunk-ablation-bench -- [--root .]

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;

/// Print a line and flush immediately — stdout is block-buffered when
/// redirected to a file, and this whole report is < 8 KB, so without an
/// explicit flush nothing would appear until process exit (the exact
/// invisibility that bit the first run).
macro_rules! sayln {
    ($($a:tt)*) => {{ println!($($a)*); let _ = std::io::stdout().flush(); }};
}

use anyhow::Result;
use hifz::code::index::{IndexOpts, index_repo};
use hifz::code::lang::Language;
use hifz::code::search::{CodeSearchOpts, search_code};
use hifz::code::splitter::CodeSplitter;
use hifz::db::{self, Db, init_schema};
use hifz::embed::Embedder;
use kernel::code_parse::walker::{WalkOpts, walk};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

#[path = "corpus_common.rs"]
mod corpus_common;
use corpus_common::Verdict;

const MIN_DOCUMENTED: usize = 25;
const K: usize = 10;
const TARGET: usize = 1000; // production DEFAULT_TARGET_CHARS (held fixed)

// --- oracle helpers: copied verbatim from code_retrieval_bench.rs ----------

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
        Some(t) => (t as i64) + 1,
        None => start_line,
    }
}

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

struct Probe {
    paraphrase: String,
    path: String,
    start: i64,
    end: i64,
    doc_top: i64,
}

// --- per-cell faithful re-index (mirrors src/code/index.rs Step 5 + 7) -----

#[derive(Debug, SurrealValue)]
struct IdRow {
    id: Option<RecordId>,
}

fn cheap_hash(s: &str) -> String {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:x}", h.finish())
}

/// Re-chunk every walked file with `CodeSplitter::new(TARGET, overlap, min)`,
/// embed, and CREATE `code_file`/`code_chunk` rows under `project` exactly as
/// `index.rs` does (edges/intel/link/snapshot omitted — retrieval doesn't
/// read them). Returns (chunk_count, total_content_bytes). Also fills
/// `cover`: per probe-index, the best coverage over `[doc_top..end]`
/// (2=full, 1=partial, 0=none) and over strict `[start..end]`.
#[allow(clippy::too_many_arguments)]
async fn index_cell(
    db: &Surreal<Db>,
    emb: &Embedder,
    project: &str,
    root: &std::path::Path,
    overlap: usize,
    probes: &[Probe],
) -> Result<(usize, usize, Vec<u8>, Vec<u8>)> {
    let files = walk(root, &WalkOpts::default())?;
    let splitter = CodeSplitter::new(TARGET, overlap);
    let mut chunk_count = 0usize;
    let mut total_bytes = 0usize;
    // per-probe best coverage
    let mut cov_corr = vec![0u8; probes.len()];
    let mut cov_strict = vec![0u8; probes.len()];
    let now = chrono::Utc::now().to_rfc3339();

    for f in &files {
        let Ok(bytes) = std::fs::read(&f.abs) else {
            continue;
        };
        let Ok(src) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let lang = Language::from_path(&f.abs).unwrap_or(Language::Plain);
        let chunks = splitter.split(lang, src).unwrap_or_default();
        if chunks.is_empty() {
            continue;
        }

        // coverage (pure; uses the real chunk line spans)
        for (pi, p) in probes.iter().enumerate() {
            if p.path != f.rel {
                continue;
            }
            for c in &chunks {
                let (cs, ce) = (c.start_line as i64, c.end_line as i64);
                // corrected span [doc_top..end]
                if cs <= p.doc_top && ce >= p.end {
                    cov_corr[pi] = 2;
                } else if cs <= p.end && p.doc_top <= ce && cov_corr[pi] < 1 {
                    cov_corr[pi] = 1;
                }
                // strict span [start..end]
                if cs <= p.start && ce >= p.end {
                    cov_strict[pi] = 2;
                } else if cs <= p.end && p.start <= ce && cov_strict[pi] < 1 {
                    cov_strict[pi] = 1;
                }
            }
        }

        // faithful insert
        chunk_count += chunks.len();
        total_bytes += chunks.iter().map(|c| c.content.len()).sum::<usize>();
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let embeddings = emb.embed_batch(&texts)?;
        let mut fr = db
            .query(
                "CREATE code_file SET project=$p, path=$rel, abs_path=$abs, \
                 language=$lang, size_bytes=$size, mtime_ns=$mtime, \
                 content_hash=$hash, chunk_count=$cc, indexed_at=$now RETURN id",
            )
            .bind(("p", project.to_string()))
            .bind(("rel", f.rel.clone()))
            .bind(("abs", f.abs.to_string_lossy().to_string()))
            .bind(("lang", lang.as_str().to_string()))
            .bind(("size", f.size_bytes as i64))
            .bind(("mtime", f.mtime_ns as i64))
            .bind(("hash", cheap_hash(src)))
            .bind(("cc", chunks.len() as i64))
            .bind(("now", now.clone()))
            .await?;
        let Some(file_id) = fr
            .take::<Vec<IdRow>>(0)
            .unwrap_or_default()
            .into_iter()
            .next()
            .and_then(|r| r.id)
        else {
            continue;
        };
        for (idx, (c, e)) in chunks.iter().zip(embeddings.into_iter()).enumerate() {
            db.query(
                "CREATE code_chunk SET file=$fid, project=$p, path=$path, \
                 language=$lang, chunk_index=$idx, content=$content, \
                 start_line=$sl, end_line=$el, start_byte=$sb, end_byte=$eb, \
                 content_hash=$ch, embedding=$emb, symbols=[], created_at=$now",
            )
            .bind(("fid", file_id.clone()))
            .bind(("p", project.to_string()))
            .bind(("path", f.rel.clone()))
            .bind(("lang", lang.as_str().to_string()))
            .bind(("idx", idx as i64))
            .bind(("content", c.content.clone()))
            .bind(("sl", c.start_line as i64))
            .bind(("el", c.end_line as i64))
            .bind(("sb", c.start_byte as i64))
            .bind(("eb", c.end_byte as i64))
            .bind(("ch", cheap_hash(&c.content)))
            .bind(("emb", e))
            .bind(("now", now.clone()))
            .await?
            .check()?;
        }
    }
    Ok((chunk_count, total_bytes, cov_corr, cov_strict))
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
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        if x == "--root" {
            root = a.next().unwrap_or(root);
        }
    }
    let root_path = std::path::Path::new(&root);
    sayln!("=== chunk-ablation-bench ({root}) — is overlap=150/min=250 justified? ===");

    let db = db::connect_mem().await?;
    let embedder = Embedder::new()?;
    init_schema(&db, embedder.dimension()).await?;

    // One default index_repo → probe set (docstring→code) + symbol spans.
    let idx = index_repo(&db, &embedder, "probe", root_path, &IndexOpts::default()).await?;
    sayln!(
        "probe index_repo: {} files, {} chunks, {} symbols",
        idx.indexed,
        idx.chunks,
        idx.symbols
    );
    if idx.symbols == 0 {
        std::process::exit(Verdict::Skip("0 symbols indexed".into()).emit());
    }
    let mut sres = db
        .query(
            "SELECT name, qualified, kind, doc, path, start_line, end_line, signature \
             FROM code_symbol WHERE project='probe' \
               AND kind IN ['function','method','struct','enum','trait']",
        )
        .await?;
    let syms: Vec<SymRow> = sres.take(0).unwrap_or_default();
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
        let Some(par) = paraphrase(&sent, s) else {
            continue;
        };
        probes.push(Probe {
            paraphrase: par,
            path: path.clone(),
            start: st,
            end: en,
            doc_top: doc_top(&root, path, st),
        });
    }
    let n = probes.len();
    sayln!("paraphrasable probes: {n}");
    if n < MIN_DOCUMENTED {
        std::process::exit(
            Verdict::Skip(format!("only {n} paraphrasable; need ≥{MIN_DOCUMENTED}")).emit(),
        );
    }

    sayln!();
    sayln!(
        "{:>7} {:>4} {:>8} {:>10}  | corrected[doc..end]  | strict[start..end] | retrieval",
        "overlap",
        "min",
        "chunks",
        "bytes"
    );
    sayln!(
        "{:>7} {:>4} {:>8} {:>10}  | full%  part%  none%  | full%  part%  none% | chunk-hit@{K}",
        "",
        "",
        "",
        ""
    );

    let opts = CodeSearchOpts {
        limit: K,
        ..Default::default()
    };
    let mut default_hit_pct = f64::NAN;
    for &overlap in &[0usize, 150, 300] {
        for &min in &[0usize, 250] {
            let proj = format!("c_o{overlap}_m{min}");
            let (chunks, bytes, cc, cs) =
                index_cell(&db, &embedder, &proj, root_path, overlap, &probes).await?;
            let (mut cf, mut cp, mut cn) = (0usize, 0usize, 0usize);
            let (mut sf, mut sp, mut sn) = (0usize, 0usize, 0usize);
            for &v in &cc {
                match v {
                    2 => cf += 1,
                    1 => cp += 1,
                    _ => cn += 1,
                }
            }
            for &v in &cs {
                match v {
                    2 => sf += 1,
                    1 => sp += 1,
                    _ => sn += 1,
                }
            }
            // retrieval: corrected chunk-hit@K of real search_code on this cell
            let mut hit = 0usize;
            for p in &probes {
                let copts = CodeSearchOpts {
                    project: Some(proj.clone()),
                    ..opts.clone()
                };
                let hits = search_code(&db, &embedder, &p.paraphrase, &copts)
                    .await
                    .unwrap_or_default();
                if hits.iter().any(|h| {
                    h.path == p.path
                        && (h.start_line as i64) <= p.end
                        && p.doc_top <= (h.end_line as i64)
                }) {
                    hit += 1;
                }
            }
            let hit_pct = pct(hit, n);
            if overlap == 150 && min == 250 {
                default_hit_pct = hit_pct;
            }
            sayln!(
                "{:>7} {:>4} {:>8} {:>10}  | {:>5.1} {:>5.1} {:>5.1}  | {:>5.1} {:>5.1} {:>5.1} | {:>5.1}% ({hit}/{n})",
                overlap,
                min,
                chunks,
                bytes,
                pct(cf, n),
                pct(cp, n),
                pct(cn, n),
                pct(sf, n),
                pct(sp, n),
                pct(sn, n),
                hit_pct,
            );
        }
    }

    // Faithfulness gate: default cell (150,250) must reproduce the canonical
    // ~97–98% (it IS production default). Window ±4pp.
    sayln!();
    if default_hit_pct.is_nan() {
        sayln!("FAITHFULNESS: n/a (default cell not run)");
    } else if (default_hit_pct - 97.7).abs() <= 4.0 {
        sayln!(
            "FAITHFULNESS GATE: PASS — default (150,250) chunk-hit {default_hit_pct:.1}% \
             reproduces production ~97.7% ⇒ re-index harness is faithful; \
             coverage AND retrieval columns trustworthy."
        );
    } else {
        sayln!(
            "FAITHFULNESS GATE: FAIL — default (150,250) chunk-hit {default_hit_pct:.1}% \
             ≠ production ~97.7% ⇒ the re-index harness diverges; RETRIEVAL column \
             UNTRUSTED. Coverage/size columns are pure (real CodeSplitter, no DB) \
             and remain trustworthy."
        );
    }
    sayln!(
        "(reading: compare the min=0 vs min=250 rows at equal overlap for the \
         drop's effect; compare overlap rows for overlap's effect. The numbers \
         decide whether 150/250 are justified — not priors.)"
    );
    Ok(())
}
