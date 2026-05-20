//! Live-reindex gate: the building blocks the file watcher drives, end-to-end
//! against an in-memory hifz. Covers the incremental single-file index path
//! (`index::index_file`), single-file delete reconciliation
//! (`gc::reconcile_deleted_file`), and the cache-fed project resolve
//! (`intel::resolve_and_persist`) — without spinning up the OS watcher.

use hifz::code::{gc, index, intel};
use hifz::db::{self, Db};
use hifz::embed::Embedder;
use std::collections::HashSet;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

const PROJECT: &str = "live-reindex";

async fn fresh() -> (Surreal<Db>, Embedder) {
    let db = db::connect_mem().await.expect("connect_mem");
    let embedder = Embedder::new().expect("embedder");
    db::init_schema(&db, embedder.dimension())
        .await
        .expect("init_schema");
    (db, embedder)
}

fn tmp_repo(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("hifz_livereindex_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (rel, body) in files {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    d
}

async fn count(db: &Surreal<Db>, sql: &str, rel: &str) -> i64 {
    #[derive(Debug, SurrealValue)]
    struct C {
        c: Option<i64>,
    }
    let mut r = db
        .query(sql)
        .bind(("p", PROJECT.to_string()))
        .bind(("r", rel.to_string()))
        .await
        .expect("count query");
    let rows: Vec<C> = r.take(0).unwrap_or_default();
    rows.into_iter().next().and_then(|x| x.c).unwrap_or(0)
}

async fn chunk_text(db: &Surreal<Db>, rel: &str) -> String {
    #[derive(Debug, SurrealValue)]
    struct Row {
        content: Option<String>,
    }
    let mut r = db
        .query("SELECT content FROM code_chunk WHERE project=$p AND path=$r")
        .bind(("p", PROJECT.to_string()))
        .bind(("r", rel.to_string()))
        .await
        .expect("chunk query");
    let rows: Vec<Row> = r.take(0).unwrap_or_default();
    rows.into_iter()
        .filter_map(|x| x.content)
        .collect::<Vec<_>>()
        .join("\n")
}

async fn symbol_exists(db: &Surreal<Db>, name: &str) -> bool {
    count(
        db,
        "SELECT count() AS c FROM code_symbol WHERE project=$p AND name=$r GROUP ALL",
        name,
    )
    .await
        > 0
}

/// `index_file` re-chunks a changed file in place (the watcher hot path that
/// keeps `code_search` — which reads `code_chunk` — fresh).
#[tokio::test]
async fn index_file_rechunks_changed_file() {
    let (db, embedder) = fresh().await;
    let repo = tmp_repo(
        "rechunk",
        &[
            ("Cargo.toml", "[package]\nname = \"demo\"\n"),
            ("src/lib.rs", "pub fn alpha() { let _ = 1; }\n"),
        ],
    );
    index::index_repo(&db, &embedder, PROJECT, &repo, &index::IndexOpts::default())
        .await
        .expect("index_repo");

    assert!(chunk_text(&db, "src/lib.rs").await.contains("alpha"));
    assert!(!chunk_text(&db, "src/lib.rs").await.contains("beta"));

    // Edit the file, then reindex just that file.
    let abs = repo.join("src/lib.rs");
    std::fs::write(&abs, "pub fn alpha() { let _ = 1; }\npub fn beta() {}\n").unwrap();
    index::index_file(&db, &embedder, PROJECT, &repo, &abs)
        .await
        .expect("index_file");

    let text = chunk_text(&db, "src/lib.rs").await;
    assert!(text.contains("beta"), "new fn not in chunks: {text}");
    // No duplicate file rows.
    let files = count(
        &db,
        "SELECT count() AS c FROM code_file WHERE project=$p AND path=$r GROUP ALL",
        "src/lib.rs",
    )
    .await;
    assert_eq!(
        files, 1,
        "index_file must upsert, not duplicate, the code_file row"
    );
}

/// `reconcile_deleted_file` purges a vanished file's chunks/symbols and
/// tombstones its row (the watcher delete path).
#[tokio::test]
async fn reconcile_deleted_file_purges_rows() {
    let (db, embedder) = fresh().await;
    let repo = tmp_repo(
        "delete",
        &[
            ("Cargo.toml", "[package]\nname = \"demo\"\n"),
            ("src/lib.rs", "pub fn gamma() {}\n"),
        ],
    );
    index::index_repo(&db, &embedder, PROJECT, &repo, &index::IndexOpts::default())
        .await
        .expect("index_repo");

    // Sanity: indexed.
    assert!(
        count(
            &db,
            "SELECT count() AS c FROM code_chunk WHERE project=$p AND path=$r GROUP ALL",
            "src/lib.rs",
        )
        .await
            > 0
    );
    assert!(symbol_exists(&db, "gamma").await);

    gc::reconcile_deleted_file(&db, PROJECT, "src/lib.rs")
        .await
        .expect("reconcile_deleted_file");

    // Chunks + symbols gone; code_file tombstoned.
    assert_eq!(
        count(
            &db,
            "SELECT count() AS c FROM code_chunk WHERE project=$p AND path=$r GROUP ALL",
            "src/lib.rs",
        )
        .await,
        0,
        "chunks should be purged"
    );
    assert!(
        !symbol_exists(&db, "gamma").await,
        "symbol should be purged"
    );
    assert_eq!(
        count(
            &db,
            "SELECT count() AS c FROM code_file \
             WHERE project=$p AND path=$r AND deleted_at IS NOT NONE GROUP ALL",
            "src/lib.rs",
        )
        .await,
        1,
        "code_file should be tombstoned"
    );
}

/// `resolve_and_persist` fed from an in-memory cache writes a newly-added
/// symbol — the incremental path the watcher runs after `index_file`, with no
/// full-repo reparse.
#[tokio::test]
async fn resolve_and_persist_writes_new_symbol_from_cache() {
    let (db, embedder) = fresh().await;
    let repo = tmp_repo(
        "resolve",
        &[
            ("Cargo.toml", "[package]\nname = \"demo\"\n"),
            ("src/lib.rs", "pub fn alpha() {}\n"),
        ],
    );
    index::index_repo(&db, &embedder, PROJECT, &repo, &index::IndexOpts::default())
        .await
        .expect("index_repo");
    assert!(symbol_exists(&db, "alpha").await);
    assert!(!symbol_exists(&db, "beta").await);

    // Edit on disk, re-chunk, rebuild the cache, then resolve scoped to the
    // changed file — exactly what the watcher worker does for one save.
    let abs = repo.join("src/lib.rs");
    std::fs::write(&abs, "pub fn alpha() {}\npub fn beta() {}\n").unwrap();
    index::index_file(&db, &embedder, PROJECT, &repo, &abs)
        .await
        .expect("index_file");

    let cache = intel::build_cache(&repo);
    let changed: HashSet<String> = HashSet::from(["src/lib.rs".to_string()]);
    intel::resolve_and_persist(&db, PROJECT, &cache, Some(&changed))
        .await
        .expect("resolve_and_persist");

    assert!(
        symbol_exists(&db, "beta").await,
        "newly added symbol must be persisted by the incremental resolve"
    );
    assert!(
        symbol_exists(&db, "alpha").await,
        "existing symbol must survive"
    );
}
