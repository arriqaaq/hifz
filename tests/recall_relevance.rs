//! Track D — deterministic relevance regression tests.
//!
//! The corpus benches (`corpus-memory-bench`, `corpus-code-bench`) answer
//! "is retrieval good on the real corpus" with variable data and gates. These
//! tests answer the orthogonal question "did a code change *break* retrieval"
//! with fixed hermetic fixtures, exact assertions, and no daemon / network /
//! LLM. They are additive — the benches stay.
//!
//! Determinism strategy: fastembed embeddings and SurrealDB BM25 are
//! reproducible for fixed input, so vector + keyword ranking is deterministic.
//! The only non-determinism is wall-clock in `rank::final_score`
//! (`exp(-age/now())`). These tests therefore assert **rank-order invariants
//! and set membership** (stable under any monotonic decay) and pin
//! time-dependent behaviour with explicit `created_at` dates or
//! `SearchConfig{ skip_recency_access: true }` — never absolute scores
//! against `now()`.

use hifz::db::{self, Db};
use hifz::embed::Embedder;
use hifz::models::SearchResult;
use hifz::search::{self, SearchConfig};
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

const PROJECT: &str = "track-d";

async fn fresh() -> (Surreal<Db>, Embedder) {
    let db = db::connect_mem().await.expect("connect_mem");
    let embedder = Embedder::new().expect("embedder");
    db::init_schema(&db, embedder.dimension())
        .await
        .expect("init_schema");
    (db, embedder)
}

/// Rank of a memory by title in a hybrid result list (mirrors
/// `memory_bench::rank_of`): first position whose `obs_type` is a memory and
/// whose title matches.
fn mem_rank(results: &[SearchResult], title: &str) -> Option<usize> {
    results
        .iter()
        .position(|r| r.obs_type.starts_with("memory:") && r.title == title)
}

async fn strength_of(db: &Surreal<Db>, title: &str) -> f64 {
    #[derive(Debug, SurrealValue)]
    struct S {
        strength: Option<f64>,
    }
    let mut r = db
        .query("SELECT strength FROM memory WHERE project=$p AND title=$t AND is_latest=true")
        .bind(("p", PROJECT.to_string()))
        .bind(("t", title.to_string()))
        .await
        .expect("query strength");
    let rows: Vec<S> = r.take(0).unwrap_or_default();
    rows.into_iter()
        .next()
        .and_then(|x| x.strength)
        .expect("memory row exists")
}

/// Write a fixture crate into a unique temp dir (mirrors the proven pattern in
/// `tests/codeintel_integration.rs::tmp_repo`).
fn tmp_repo(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "hifz_trackd_{name}_{}_{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    for (rel, src) in files {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    d
}

/// Raw BM25-only ranking over `code_chunk` — the exact full-text branch from
/// `src/code/search.rs` (no vector fusion). Used as the baseline arm.
async fn bm25_code_paths(db: &Surreal<Db>, query: &str, limit: usize) -> Vec<String> {
    #[derive(Debug, SurrealValue)]
    struct H {
        path: Option<String>,
        ft: Option<f64>,
    }
    let sql = format!(
        "SELECT path, search::score(1) AS ft FROM code_chunk \
         WHERE content @1@ $q AND (project = $p OR project = 'global') \
         ORDER BY ft DESC LIMIT {limit}"
    );
    let mut r = db
        .query(&sql)
        .bind(("q", query.to_string()))
        .bind(("p", PROJECT.to_string()))
        .await
        .expect("bm25 query");
    let rows: Vec<H> = r.take(0).unwrap_or_default();
    rows.into_iter().filter_map(|h| h.path).collect()
}

// ---------------------------------------------------------------------------
// 1. Code search returns the right symbol for a natural-language query.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn code_search_returns_target_symbol() {
    let (db, embedder) = fresh().await;
    let repo = tmp_repo(
        "codetarget",
        &[(
            "src/lib.rs",
            "/// Read and parse a TOML configuration file from disk.\n\
                 pub fn load_toml_config(path: &str) -> String { String::new() }\n\n\
                 /// Compute the SHA256 digest of a byte buffer.\n\
                 pub fn sha256_digest(bytes: &[u8]) -> [u8; 32] { [0u8; 32] }\n\n\
                 /// Send an HTTP GET request and return the response body.\n\
                 pub fn http_get(url: &str) -> String { String::new() }\n",
        )],
    );
    hifz::code::index::index_repo(
        &db,
        &embedder,
        PROJECT,
        &repo,
        &hifz::code::index::IndexOpts::default(),
    )
    .await
    .expect("index_repo");

    let opts = hifz::code::search::CodeSearchOpts {
        limit: 5,
        project: Some(PROJECT.to_string()),
        ..Default::default()
    };
    let hits = hifz::code::search::search_code(
        &db,
        &embedder,
        "read configuration settings from a TOML file",
        &opts,
    )
    .await
    .expect("search_code");

    assert!(!hits.is_empty(), "expected code-search hits");
    // The config loader's chunk must rank first for a config-intent query.
    assert!(
        hits[0].snippet.contains("load_toml_config"),
        "rank-1 hit should be the config loader, got: {}",
        hits[0].snippet
    );

    // A semantically unrelated query must NOT rank the config loader first.
    let unrelated = hifz::code::search::search_code(
        &db,
        &embedder,
        "compute a cryptographic hash of raw bytes",
        &opts,
    )
    .await
    .expect("search_code 2");
    assert!(
        unrelated[0].snippet.contains("sha256_digest"),
        "hash-intent query should surface sha256_digest first, got: {}",
        unrelated[0].snippet
    );
}

// ---------------------------------------------------------------------------
// 2. Hybrid (vector) beats BM25-only when the query shares no identifier
//    tokens with the code body — proves the embedding adds real value.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn code_search_hybrid_beats_bm25_on_paraphrase() {
    let (db, embedder) = fresh().await;
    // Function whose identifiers deliberately share NO tokens with the query
    // "remove outdated records". Body talks about pruning; query is a synonym.
    let repo = tmp_repo(
        "paraphrase",
        &[
            (
                "src/a.rs",
                "pub fn obliterate_stale_entries(v: &mut Vec<i64>) { v.clear(); }\n",
            ),
            (
                "src/b.rs",
                "pub fn render_html_template(ctx: &str) -> String { ctx.to_string() }\n",
            ),
            (
                "src/c.rs",
                "pub fn open_tcp_socket(addr: &str) -> bool { !addr.is_empty() }\n",
            ),
        ],
    );
    hifz::code::index::index_repo(
        &db,
        &embedder,
        PROJECT,
        &repo,
        &hifz::code::index::IndexOpts::default(),
    )
    .await
    .expect("index_repo");

    let query = "delete outdated records";
    let opts = hifz::code::search::CodeSearchOpts {
        limit: 5,
        project: Some(PROJECT.to_string()),
        ..Default::default()
    };
    let hybrid = hifz::code::search::search_code(&db, &embedder, query, &opts)
        .await
        .expect("search_code");
    let hybrid_rank = hybrid.iter().position(|h| h.path.ends_with("a.rs"));
    let bm25 = bm25_code_paths(&db, query, 5).await;
    let bm25_rank = bm25.iter().position(|p| p.ends_with("a.rs"));

    assert!(
        matches!(hybrid_rank, Some(r) if r < 3),
        "hybrid must surface the paraphrased target in top-3 (got {hybrid_rank:?}); \
         hybrid={:?}",
        hybrid.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
    // BM25-only has no lexical overlap to latch onto → it must do strictly
    // worse (miss it, or rank it below where hybrid put it).
    let bm25_worse = match (bm25_rank, hybrid_rank) {
        (None, _) => true,
        (Some(b), Some(h)) => b > h,
        _ => false,
    };
    assert!(
        bm25_worse,
        "embedding should beat BM25-only on a no-overlap paraphrase \
         (bm25_rank={bm25_rank:?}, hybrid_rank={hybrid_rank:?})"
    );
}

// ---------------------------------------------------------------------------
// 3. The injection path surfaces the topically-relevant memory, and the
//    decoy ranks below it in hybrid search.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn injection_surfaces_grounded_memory() {
    let (db, embedder) = fresh().await;
    let relevant = "Auth tokens are validated by an Axum extractor";
    let decoy = "The CI pipeline runs cargo clippy on every push";
    hifz::remember::save(
        &db,
        &embedder,
        PROJECT,
        "decision",
        relevant,
        "Every handler enforces auth via a FromRequestParts extractor that checks the bearer token.",
        &[],
        &["src/auth.rs".to_string()],
        None,
    )
    .await
    .expect("save relevant");
    hifz::remember::save(
        &db,
        &embedder,
        PROJECT,
        "decision",
        decoy,
        "GitHub Actions runs clippy and rustfmt as required status checks.",
        &[],
        &[".github/workflows/ci.yml".to_string()],
        None,
    )
    .await
    .expect("save decoy");

    let query = "how is authentication enforced on requests";

    // Injection path (what the recall skill feeds the LLM). No `run` rows are
    // inserted, so the `# Recent task outcomes` section is absent and cannot
    // leak the title via a lesson — this measures the memory path only.
    let ctx = hifz::context::generate_context_with_query(
        &db,
        Some(&embedder),
        PROJECT,
        Some(query),
        2048,
    )
    .await
    .expect("generate_context_with_query");
    assert!(
        ctx.contains(relevant),
        "injected context must contain the auth memory title.\n--- context ---\n{ctx}"
    );

    let results = search::search_hybrid(&db, &embedder, query, 10, Some(PROJECT))
        .await
        .expect("search_hybrid");
    let rr = mem_rank(&results, relevant);
    let dr = mem_rank(&results, decoy);
    assert!(rr.is_some(), "relevant memory must be retrieved");
    assert!(
        match (rr, dr) {
            (Some(r), Some(d)) => r < d,
            (Some(_), None) => true,
            _ => false,
        },
        "relevant must outrank decoy (relevant={rr:?}, decoy={dr:?})"
    );
}

// ---------------------------------------------------------------------------
// 4. Commit-grounding reranks: a non-revert commit on a memory's file
//    strengthens it (and a revert weakens it). Locks the project's one
//    defensible axis with exact assertions.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn commit_grounding_reranks() {
    let (db, embedder) = fresh().await;
    // Two near-equally-relevant memories for the probe; they differ only by
    // which file they touch, so grounding one must move the needle.
    let shipped = "Storage uses an embedded SurrealKV engine";
    let reverted = "Storage uses a RocksDB-backed engine";
    hifz::remember::save(
        &db,
        &embedder,
        PROJECT,
        "decision",
        shipped,
        "The store is an embedded SurrealKV instance; no external DB process.",
        &[],
        &["src/store_kv.rs".to_string()],
        None,
    )
    .await
    .expect("save shipped");
    hifz::remember::save(
        &db,
        &embedder,
        PROJECT,
        "decision",
        reverted,
        "An earlier attempt used a RocksDB-backed store.",
        &[],
        &["src/store_rocks.rs".to_string()],
        None,
    )
    .await
    .expect("save reverted");

    // `remember::save` starts strength at the 1.0 ceiling and grounding
    // clamps `strength * boost` to 1.0, so a boost there is a silent no-op.
    // Give both rows headroom so the mechanism (boost up / revert down) is
    // observable and the assertion is meaningful.
    db.query("UPDATE memory SET strength = 0.5 WHERE project = $p")
        .bind(("p", PROJECT.to_string()))
        .await
        .expect("set headroom");

    let s0 = strength_of(&db, shipped).await;

    // A real (non-revert) commit touching the shipped file → strengthen.
    hifz::ground::on_commit_signal(
        &db,
        PROJECT,
        &hifz::ground::CommitSignal {
            files_added_modified: vec!["src/store_kv.rs".to_string()],
            files_removed: vec![],
            is_revert: false,
        },
    )
    .await
    .expect("ground confirm");
    let s1 = strength_of(&db, shipped).await;
    assert!(
        s1 > s0,
        "a non-revert commit on the memory's file must raise strength ({s0} -> {s1})"
    );

    // A revert touching the reverted approach's file → weaken it.
    let r0 = strength_of(&db, reverted).await;
    hifz::ground::on_commit_signal(
        &db,
        PROJECT,
        &hifz::ground::CommitSignal {
            files_added_modified: vec!["src/store_rocks.rs".to_string()],
            files_removed: vec![],
            is_revert: true,
        },
    )
    .await
    .expect("ground revert");
    let r1 = strength_of(&db, reverted).await;
    assert!(
        r1 < r0,
        "a revert touching the memory's file must lower strength ({r0} -> {r1})"
    );

    // Net effect on ranking: the shipped approach must not rank below the
    // reverted one for a storage-engine query.
    let results = search::search_hybrid(
        &db,
        &embedder,
        "what storage engine does the project use",
        10,
        Some(PROJECT),
    )
    .await
    .expect("search_hybrid");
    match (mem_rank(&results, shipped), mem_rank(&results, reverted)) {
        (Some(sr), Some(rr)) => assert!(
            sr <= rr,
            "after grounding, shipped ({sr}) must not rank below reverted ({rr})"
        ),
        (Some(_), None) => {}
        (None, _) => panic!("shipped memory missing from results"),
    }
}

// ---------------------------------------------------------------------------
// 5. A still-relevant old memory survives the recency decay (regression
//    guard that exp(-age/30) does not bury long-horizon memory).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn long_horizon_memory_still_recallable() {
    let (db, embedder) = fresh().await;
    let old = "Embeddings are produced locally with fastembed at 384 dimensions";
    let decoy = "The website is built with SvelteKit and Tailwind";
    hifz::remember::save(
        &db,
        &embedder,
        PROJECT,
        "decision",
        old,
        "Vector embeddings use fastembed AllMiniLM (384-d), fully offline, no network calls.",
        &[],
        &["src/embed.rs".to_string()],
        None,
    )
    .await
    .expect("save old");
    hifz::remember::save(
        &db,
        &embedder,
        PROJECT,
        "decision",
        decoy,
        "Frontend stack is SvelteKit with Tailwind CSS.",
        &[],
        &["website/src/app.css".to_string()],
        None,
    )
    .await
    .expect("save decoy");

    // Backdate the relevant memory ~60 days (decay factor ≈ e^-2 ≈ 0.135).
    let old_ts = "2026-03-19T00:00:00+00:00"; // ~60d before the 2026-05-18 corpus epoch
    db.query("UPDATE memory SET created_at=$ts, updated_at=$ts WHERE project=$p AND title=$t")
        .bind(("ts", old_ts.to_string()))
        .bind(("p", PROJECT.to_string()))
        .bind(("t", old.to_string()))
        .await
        .expect("backdate");

    let query = "how are vector embeddings generated";

    // Clock removed: pure relevance must put the old memory first.
    let mut cfg = SearchConfig::default();
    cfg.skip_recency_access = true;
    let pure = search::search_hybrid_with_config(&db, &embedder, query, 10, Some(PROJECT), cfg)
        .await
        .expect("search no-decay");
    assert!(
        matches!(mem_rank(&pure, old), Some(r) if r <= mem_rank(&pure, decoy).unwrap_or(usize::MAX)),
        "without decay the relevant old memory must outrank the decoy"
    );

    // Decay ON: the 60-day-old but highly-relevant memory must still be
    // retrievable in the top-K (it must not be buried by Ebbinghaus decay).
    let decayed = search::search_hybrid(&db, &embedder, query, 10, Some(PROJECT))
        .await
        .expect("search decay");
    assert!(
        matches!(mem_rank(&decayed, old), Some(r) if r < 5),
        "a 60-day-old still-relevant memory must remain in the top-5 with decay on; \
         ranks={:?}",
        decayed
            .iter()
            .filter(|r| r.obs_type.starts_with("memory:"))
            .map(|r| &r.title)
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// 6. Static guard: the commit-grounded oracle must never read
//    `run.recalled_ids` (that would make the eval circular — it IS the
//    system's own retrieval output).
// ---------------------------------------------------------------------------
#[test]
fn corpus_memory_bench_oracle_is_non_circular() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/benchmark/corpus_memory_bench.rs"
    );
    match std::fs::read_to_string(path) {
        Ok(src) => {
            // Allow the substring inside comments that explain *why* it is not
            // used; forbid it in any non-comment line (an actual field read).
            let offending: Vec<(usize, &str)> = src
                .lines()
                .enumerate()
                .filter(|(_, l)| {
                    let t = l.trim_start();
                    !t.starts_with("//") && !t.starts_with("*") && l.contains("recalled_ids")
                })
                .map(|(i, l)| (i + 1, l))
                .collect();
            assert!(
                offending.is_empty(),
                "corpus_memory_bench.rs must not read run.recalled_ids \
                 (circular oracle). Offending lines: {offending:?}"
            );
        }
        // Bench not present yet (e.g. running this test in isolation before
        // Track B lands): nothing to guard.
        Err(_) => {}
    }
}
