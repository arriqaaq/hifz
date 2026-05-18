//! E4 gate: the project-wide code-intel pass, end-to-end against an
//! in-memory hifz. Proves the properties the old `.scm` path could not:
//! scope-qualified distinct identity, idempotent reindex, `references_symbol`
//! survival across reindex (no re-anchor needed), structural rename
//! reconciliation, and the 3-state `resolution` (nothing dropped).

use hifz::Hifz;
use hifz::models::CodeIndexReq;
use surrealdb::types::{RecordId, SurrealValue};

fn tmp_repo(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("hifz_e4_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    d
}

#[derive(Debug, SurrealValue)]
struct QRow {
    qualified: Option<String>,
}
#[derive(Debug, SurrealValue)]
struct IdRow {
    id: Option<RecordId>,
}

async fn quals(h: &Hifz, name: &str) -> Vec<String> {
    let mut r =
        h.db.query("SELECT qualified FROM code_symbol WHERE project='demo' AND name=$n")
            .bind(("n", name.to_string()))
            .await
            .unwrap();
    let rows: Vec<QRow> = r.take(0).unwrap_or_default();
    let mut v: Vec<String> = rows.into_iter().filter_map(|x| x.qualified).collect();
    v.sort();
    v
}

async fn sym_id(h: &Hifz, qualified: &str) -> RecordId {
    let mut r =
        h.db.query("SELECT id FROM code_symbol WHERE project='demo' AND qualified=$q")
            .bind(("q", qualified.to_string()))
            .await
            .unwrap();
    let rows: Vec<IdRow> = r.take(0).unwrap_or_default();
    rows.into_iter()
        .next()
        .and_then(|x| x.id)
        .expect("symbol exists")
}

async fn count(h: &Hifz, sql: &str) -> i64 {
    #[derive(Debug, SurrealValue)]
    struct C {
        c: Option<i64>,
    }
    let mut r = h.db.query(sql).await.unwrap();
    let rows: Vec<C> = r.take(0).unwrap_or_default();
    rows.into_iter().next().and_then(|x| x.c).unwrap_or(0)
}

#[tokio::test]
async fn e4_codeintel_end_to_end() {
    let h = Hifz::open_memory().await.expect("open in-memory hifz");
    let repo = tmp_repo("e2e");
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub mod util;\n\
         pub struct A;\n\
         pub struct B;\n\
         impl A { pub fn run(&self) { util::helper(); } }\n\
         impl B { pub fn run(&self) {} }\n\
         pub fn run() {}\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/util.rs"),
        "pub fn helper() { external_thing(); }\n",
    )
    .unwrap();

    let req = CodeIndexReq {
        project: "demo".into(),
        root: repo.to_string_lossy().to_string(),
        follow_symlinks: None,
        max_file_bytes: None,
    };
    h.code_index(req.clone()).await.expect("index 1");

    // 1. Collision fix: three distinct `run` identities.
    let runs = quals(&h, "run").await;
    assert_eq!(
        runs,
        vec![
            "demo::A::run".to_string(),
            "demo::B::run".to_string(),
            "demo::run".to_string()
        ],
        "scope-qualified distinct ids"
    );

    // 2. Edges with resolution: a resolved cross-file call + an external.
    let resolved_calls = count(
        &h,
        "SELECT count() AS c FROM edge WHERE relation='calls' AND resolution='resolved' GROUP ALL",
    )
    .await;
    assert!(resolved_calls >= 1, "A::run → util::helper resolved");
    let externals = count(
        &h,
        "SELECT count() AS c FROM edge WHERE resolution='external' GROUP ALL",
    )
    .await;
    assert!(
        externals >= 1,
        "external_thing() recorded as external, not dropped"
    );
    let contains = count(
        &h,
        "SELECT count() AS c FROM edge WHERE relation='contains' AND via='code' GROUP ALL",
    )
    .await;
    assert!(contains >= 2, "impl/struct contains methods");

    // 3. Idempotent reindex: stable deterministic id, no churn.
    let id_before = sym_id(&h, "demo::util::helper").await;
    let n_before = count(&h, "SELECT count() AS c FROM code_symbol GROUP ALL").await;
    h.code_index(req.clone())
        .await
        .expect("index 2 (idempotent)");
    let id_after = sym_id(&h, "demo::util::helper").await;
    let n_after = count(&h, "SELECT count() AS c FROM code_symbol GROUP ALL").await;
    assert_eq!(id_before, id_after, "symbol id stable across reindex");
    assert_eq!(n_before, n_after, "no symbol-row churn on reindex");

    // 4. references_symbol survives reindex (the headline E4 win — no
    //    re-anchor). Attach a synthetic memory→symbol edge, reindex, assert
    //    it still points at the (same-id) symbol.
    let mut mr =
        h.db.query(
            "CREATE memory SET project='demo', category='note', title='t', \
             content='c', keywords=[], files=[], tags=[], strength=1.0, \
             created_at='2026-01-01', updated_at='2026-01-01' RETURN id",
        )
        .await
        .unwrap();
    let mem_id = mr
        .take::<Vec<IdRow>>(0)
        .unwrap()
        .into_iter()
        .next()
        .and_then(|x| x.id)
        .expect("memory id");
    h.db.query(
        "RELATE $m->edge->$sid SET relation='references_symbol', \
             via='test', score=0.9, created_at='2026-01-01'",
    )
    .bind(("m", mem_id))
    .bind(("sid", id_after.clone()))
    .await
    .unwrap();
    h.code_index(req.clone()).await.expect("index 3");
    let surv = count(
        &h,
        "SELECT count() AS c FROM edge WHERE relation='references_symbol' \
         AND out IN (SELECT VALUE id FROM code_symbol WHERE qualified='demo::util::helper') GROUP ALL",
    )
    .await;
    assert_eq!(
        surv, 1,
        "references_symbol edge survived reindex via stable id"
    );

    // 5. Structural rename reconciliation: rename `helper` → `helper2`
    //    (identical body) → inbound references_symbol migrates.
    std::fs::write(
        repo.join("src/util.rs"),
        "pub fn helper2() { external_thing(); }\n",
    )
    .unwrap();
    h.code_index(req.clone()).await.expect("index 4 (rename)");
    let migrated = count(
        &h,
        "SELECT count() AS c FROM edge WHERE relation='references_symbol' \
         AND out IN (SELECT VALUE id FROM code_symbol WHERE qualified='demo::util::helper2') GROUP ALL",
    )
    .await;
    assert_eq!(migrated, 1, "rename reconciled: edge migrated to helper2");
    let orphaned = count(
        &h,
        "SELECT count() AS c FROM code_symbol WHERE qualified='demo::util::helper' GROUP ALL",
    )
    .await;
    assert_eq!(orphaned, 0, "old symbol removed after rename");

    let _ = std::fs::remove_dir_all(&repo);
}

#[derive(Debug, SurrealValue)]
struct DocRow {
    doc: Option<String>,
}

/// Doc-extraction gate: `code_symbol.doc` must be populated from real source
/// comments across languages (Rust `///`, Python `"""`, JS `/** */`).
/// Regression guard for the pipeline gap where `doc` was always NONE.
#[tokio::test]
async fn doc_extraction_rust_python_js() {
    let h = Hifz::open_memory().await.expect("open in-memory hifz");
    let repo = tmp_repo("docx");
    std::fs::write(
        repo.join("src/lib.rs"),
        "/// Adds two numbers and returns the sum.\n\
         pub fn add_two(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/util.py"),
        "def greet(name):\n    \"\"\"Return a friendly greeting for the given name.\"\"\"\n    return name\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/util.js"),
        "/** Multiplies the input by two. */\nexport function double(x) { return x * 2; }\n",
    )
    .unwrap();

    let req = CodeIndexReq {
        project: "docx".into(),
        root: repo.to_string_lossy().to_string(),
        follow_symlinks: None,
        max_file_bytes: None,
    };
    h.code_index(req).await.expect("index docx");

    async fn doc_of(h: &Hifz, name: &str) -> String {
        let mut r =
            h.db.query("SELECT doc FROM code_symbol WHERE project='docx' AND name=$n")
                .bind(("n", name.to_string()))
                .await
                .unwrap();
        let rows: Vec<DocRow> = r.take(0).unwrap_or_default();
        rows.into_iter()
            .filter_map(|x| x.doc)
            .find(|d| !d.trim().is_empty())
            .unwrap_or_default()
    }

    let rust_doc = doc_of(&h, "add_two").await;
    assert!(
        rust_doc.contains("Adds two numbers"),
        "Rust /// doc not extracted, got: {rust_doc:?}"
    );
    let py_doc = doc_of(&h, "greet").await;
    assert!(
        py_doc.contains("friendly greeting"),
        "Python docstring not extracted, got: {py_doc:?}"
    );
    let js_doc = doc_of(&h, "double").await;
    assert!(
        js_doc.contains("Multiplies the input"),
        "JS /** */ doc not extracted, got: {js_doc:?}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
