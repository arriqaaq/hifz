//! Track C — graphify feature-parity + comparable token-reduction benchmark.
//!
//! graphify's one measurable public claim is "71.5x token reduction — query
//! the graph instead of grepping through files". This reproduces graphify's
//! exact methodology (graphify/benchmark.py): tokens ≈ max(1, chars/4),
//! reduction_ratio = corpus_tokens / avg_query_tokens — so the hifz number
//! sits directly next to graphify's. It also asserts language coverage for
//! every language hifz claims to index (a regression fails loudly).
//!
//! The qualitative capability matrix lives in docs/eval/graphify-parity.md.
//!
//! Usage: cargo run --release --bin parity-bench [-- --root <dir>]

use anyhow::Result;
use hifz::code::index::{IndexOpts, index_repo};
use hifz::code::search::{CodeSearchOpts, search_code};
use hifz::db::{self, init_schema};
use hifz::embed::Embedder;
use surrealdb::types::SurrealValue;

#[path = "corpus_common.rs"]
mod corpus_common;
use corpus_common::Verdict;

/// graphify/benchmark.py: `_estimate_tokens = max(1, len(text)//4)`.
fn est_tokens(s: &str) -> usize {
    (s.len() / 4).max(1)
}

/// Representative architecture questions an agent would otherwise answer by
/// reading/grepping the whole codebase.
const QUESTIONS: &[&str] = &[
    "how is hybrid search ranking implemented",
    "how are embeddings generated",
    "how does the database schema define memory",
    "how is commit grounding applied to memories",
    "how are git commits detected and observed",
    "how is the knowledge graph expanded during search",
];

/// Languages hifz claims to index (tree-sitter set), with a minimal fixture
/// that must yield ≥1 symbol.
const LANG_FIXTURES: &[(&str, &str, &str)] = &[
    ("rust", "l.rs", "/// doc\npub fn alpha_one() -> i32 { 1 }\n"),
    (
        "python",
        "l.py",
        "def beta_two():\n    \"\"\"doc\"\"\"\n    return 2\n",
    ),
    (
        "typescript",
        "l.ts",
        "/** doc */\nexport function gammaThree(): number { return 3; }\n",
    ),
    (
        "javascript",
        "l.js",
        "/** doc */\nexport function deltaFour() { return 4; }\n",
    ),
    (
        "go",
        "l.go",
        "package p\n// doc\nfunc EpsilonFive() int { return 5 }\n",
    ),
    (
        "java",
        "L.java",
        "class L { /** doc */ public int zetaSix() { return 6; } }\n",
    ),
    ("c", "l.c", "/* doc */\nint eta_seven(void) { return 7; }\n"),
    (
        "cpp",
        "l.cpp",
        "/* doc */\nint theta_eight() { return 8; }\n",
    ),
];

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("hifz_parity_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    d
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mut root = "crates/kernel".to_string();
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        if x == "--root" {
            root = a.next().unwrap_or(root);
        }
    }

    println!("=== parity-bench: hifz vs graphify ===");
    let db = db::connect_mem().await?;
    let embedder = Embedder::new()?;
    init_schema(&db, embedder.dimension()).await?;
    let report = index_repo(
        &db,
        &embedder,
        "parity",
        std::path::Path::new(&root),
        &IndexOpts::default(),
    )
    .await?;
    println!(
        "indexed {root}: {} files, {} chunks, {} symbols",
        report.indexed, report.chunks, report.symbols
    );
    if report.symbols == 0 {
        std::process::exit(Verdict::Skip("0 symbols indexed".into()).emit());
    }

    // Corpus tokens = the raw source an agent would read to answer (every
    // indexed chunk's content). graphify counts the corpus the same way.
    #[derive(Debug, SurrealValue)]
    struct C {
        content: Option<String>,
    }
    let mut cr = db
        .query("SELECT content FROM code_chunk WHERE project='parity'")
        .await?;
    let chunks: Vec<C> = cr.take(0).unwrap_or_default();
    let corpus_tokens: usize = chunks
        .iter()
        .filter_map(|c| c.content.as_deref())
        .map(est_tokens)
        .sum();

    // Per-question hifz query tokens = what code_search actually returns
    // (the bounded context the agent reads instead of the corpus).
    let opts = CodeSearchOpts {
        limit: 8,
        project: Some("parity".to_string()),
        ..Default::default()
    };
    let mut q_tokens = Vec::new();
    println!();
    println!("per-question token reduction:");
    for q in QUESTIONS {
        let hits = search_code(&db, &embedder, q, &opts).await?;
        let t: usize = hits
            .iter()
            .map(|h| est_tokens(&h.snippet))
            .sum::<usize>()
            .max(1);
        q_tokens.push(t);
        println!("  [{:>5.1}x] {}", corpus_tokens as f64 / t as f64, q);
    }
    let avg_q = q_tokens.iter().sum::<usize>() as f64 / q_tokens.len() as f64;
    let reduction = corpus_tokens as f64 / avg_q;
    println!();
    println!("corpus tokens     : {corpus_tokens}");
    println!("avg query tokens  : {avg_q:.0}");
    println!(
        "reduction ratio   : {reduction:.1}x  (graphify methodology; their headline: 71.5x \
         on a 52-file multimodal corpus)"
    );

    // Language coverage: every claimed language must index ≥1 symbol.
    println!();
    println!("language coverage (claimed tree-sitter set):");
    let mut missing = Vec::new();
    for (lang, fname, src) in LANG_FIXTURES {
        let d = tmp(lang);
        std::fs::write(d.join("src").join(fname), src).unwrap();
        let ldb = db::connect_mem().await?;
        init_schema(&ldb, embedder.dimension()).await?;
        let rep = index_repo(&ldb, &embedder, "lc", &d, &IndexOpts::default()).await?;
        let ok = rep.symbols > 0;
        println!(
            "  {lang:<11} {}  ({} symbols)",
            if ok { "OK " } else { "MISS" },
            rep.symbols
        );
        if !ok {
            missing.push(*lang);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    println!();
    println!("see docs/eval/graphify-parity.md for the full capability matrix");
    println!();

    const MIN_REDUCTION: f64 = 5.0; // graphify's small-corpus floor
    let mut reasons = Vec::new();
    if reduction < MIN_REDUCTION {
        reasons.push(format!(
            "token reduction {reduction:.1}x below floor {MIN_REDUCTION}x"
        ));
    }
    if !missing.is_empty() {
        reasons.push(format!(
            "claimed languages produced 0 symbols: {}",
            missing.join(", ")
        ));
    }
    let v = if reasons.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail(reasons)
    };
    std::process::exit(v.emit())
}
