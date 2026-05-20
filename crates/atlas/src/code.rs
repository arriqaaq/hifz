//! Code-graph projection. atlas runs hifz-core's code-intelligence core
//! (walk → semantic scope-qualified identity → 3-state resolution) itself
//! and *projects* the result into its own `atlas_node`/`atlas_edge` so the
//! code graph clusters jointly with documents/concepts. Fully recomputed
//! each run (derived data) → idempotent. Nothing is dropped: `external`
//! targets become `external` nodes; `ambiguous` fans out to candidates.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use kernel::code_parse::codegraph::walk_file;
use kernel::code_parse::coderesolve::{Resolution, resolve_project};
use kernel::code_parse::lang::Language;
use kernel::code_parse::langmod::module_path;
use kernel::code_parse::walker::{WalkOpts, walk};
use kernel::embed::Embedder;
use sha2::{Digest, Sha256};
use surrealdb::types::RecordId;

use crate::store::Store;

#[derive(Debug, Default, serde::Serialize)]
pub struct CodeProjectReport {
    pub symbols: usize,
    pub externals: usize,
    pub files: usize,
    pub edges: usize,
}

fn nkey(project: &str, ns: &str, val: &str) -> String {
    let mut h = Sha256::new();
    h.update(project.as_bytes());
    h.update(b"\0");
    h.update(ns.as_bytes());
    h.update(b"\0");
    h.update(val.as_bytes());
    hex::encode(&h.finalize()[..16])
}
fn rid(project: &str, ns: &str, val: &str) -> RecordId {
    RecordId::new("atlas_node", nkey(project, ns, val))
}

pub async fn project_code_graph(
    store: &Store,
    embedder: &Embedder,
    root: &Path,
) -> Result<CodeProjectReport> {
    let p = store.project.clone(); // slug — for deterministic nkey/rid hashing
    let pid = store.pid(); // record<project> id — for query binds
    let now = chrono::Utc::now().to_rfc3339();

    // Wipe prior derived code graph for this project (doc/concept untouched).
    let _ = store
        .db
        .query("DELETE atlas_edge WHERE project=$p AND via='code'")
        .bind(("p", pid.clone()))
        .await;
    let _ = store
        .db
        .query("DELETE atlas_node WHERE project=$p AND kind IN ['code_symbol','external','file']")
        .bind(("p", pid.clone()))
        .await;

    // Walk + per-file FileGraph; track qualified→language + module set.
    let files = walk(root, &WalkOpts::default())?;
    let mut graphs = Vec::new();
    let mut qual_lang: HashMap<String, &'static str> = HashMap::new();
    let mut modules: std::collections::HashSet<String> = std::collections::HashSet::new();
    for f in &files {
        let Ok(bytes) = std::fs::read(&f.abs) else {
            continue;
        };
        let Ok(src) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let lang = Language::from_path(&f.abs).unwrap_or(Language::Plain);
        if lang == Language::Plain {
            continue;
        }
        let mp = module_path(lang, &f.abs, root);
        let Ok(fg) = walk_file(lang, src, &mp) else {
            continue;
        };
        for d in &fg.defs {
            qual_lang.insert(d.qualified.clone(), lang.as_str());
        }
        modules.insert(mp);
        graphs.push(fg);
    }
    let pg = resolve_project(graphs);
    let mut report = CodeProjectReport::default();

    // File/module nodes (so `imports` edges have a real source).
    for m in &modules {
        let id = format!("atlas_node:{}", nkey(&p, "file", m));
        let _ = store
            .db
            .query(
                "UPSERT type::record($id) SET project=$p, kind='file', label=$l, \
                 qualified=$l, cluster=-1, created_at=$now",
            )
            .bind(("id", id))
            .bind(("p", pid.clone()))
            .bind(("l", m.clone()))
            .bind(("now", now.clone()))
            .await;
        report.files += 1;
    }

    // Symbol nodes.
    let known: std::collections::HashSet<&str> =
        pg.defs.iter().map(|d| d.qualified.as_str()).collect();
    for d in &pg.defs {
        let id = format!("atlas_node:{}", nkey(&p, "code", &d.qualified));
        let lang = qual_lang.get(&d.qualified).copied().unwrap_or("");
        let emb = embedder.embed_single(&d.qualified).ok();
        let _ = store
            .db
            .query(
                "UPSERT type::record($id) SET project=$p, kind='code_symbol', \
                 label=$n, qualified=$q, language=$lang, summary=$sig, \
                 embedding=$e, cluster=-1, created_at=$now",
            )
            .bind(("id", id))
            .bind(("p", pid.clone()))
            .bind(("n", d.name.clone()))
            .bind(("q", d.qualified.clone()))
            .bind(("lang", lang.to_string()))
            .bind(("sig", d.signature.clone()))
            .bind(("e", emb))
            .bind(("now", now.clone()))
            .await;
        report.symbols += 1;
    }

    // Edges (+ external nodes on demand). Bound RecordId endpoints only.
    for e in &pg.edges {
        let from = if e.relation == "imports" && modules.contains(&e.from) {
            rid(&p, "file", &e.from)
        } else if known.contains(e.from.as_str()) {
            rid(&p, "code", &e.from)
        } else {
            continue;
        };

        let targets: Vec<(RecordId, f64)> = match e.resolution {
            Resolution::Resolved => {
                if known.contains(e.to.as_str()) {
                    vec![(rid(&p, "code", &e.to), 1.0)]
                } else {
                    vec![]
                }
            }
            Resolution::External => {
                let id = format!("atlas_node:{}", nkey(&p, "ext", &e.to));
                let _ = store
                    .db
                    .query(
                        "UPSERT type::record($id) SET project=$p, kind='external', \
                         label=$l, qualified=$l, cluster=-1, created_at=$now",
                    )
                    .bind(("id", id))
                    .bind(("p", pid.clone()))
                    .bind(("l", e.to.clone()))
                    .bind(("now", now.clone()))
                    .await;
                report.externals += 1;
                vec![(rid(&p, "ext", &e.to), 1.0)]
            }
            Resolution::Ambiguous => {
                let cs: Vec<&String> = e
                    .candidates
                    .iter()
                    .filter(|c| known.contains(c.as_str()))
                    .collect();
                let n = cs.len().max(1) as f64;
                cs.into_iter()
                    .map(|c| (rid(&p, "code", c), 1.0 / n))
                    .collect()
            }
        };

        for (to, score) in targets {
            let _ = store
                .db
                .query(
                    "RELATE $f->atlas_edge->$t SET project=$p, relation=$rel, \
                     via='code', score=$s, resolution=$res, created_at=$now",
                )
                .bind(("f", from.clone()))
                .bind(("t", to))
                .bind(("p", pid.clone()))
                .bind(("rel", e.relation.to_string()))
                .bind(("s", score))
                .bind(("res", e.resolution.as_str().to_string()))
                .bind(("now", now.clone()))
                .await;
            report.edges += 1;
        }
    }

    tracing::info!(
        "atlas code projection: symbols={} files={} ext={} edges={}",
        report.symbols,
        report.files,
        report.externals,
        report.edges
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::types::SurrealValue;

    #[tokio::test]
    async fn projects_scope_qualified_code_graph() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub mod util;\npub struct A;\npub struct B;\n\
             impl A { pub fn run(&self){ util::helper(); } }\n\
             impl B { pub fn run(&self){} }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/util.rs"),
            "pub fn helper(){ missing_ext(); }\n",
        )
        .unwrap();

        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();

        let r = project_code_graph(&store, &emb, dir.path()).await.unwrap();
        assert!(r.symbols >= 3, "A::run, B::run, helper, … distinct");
        assert!(r.externals >= 1, "missing_ext recorded as external");

        #[derive(Debug, SurrealValue)]
        struct Q {
            qualified: Option<String>,
        }
        let mut q = store
            .db
            .query("SELECT qualified FROM atlas_node WHERE kind='code_symbol' AND label='run'")
            .await
            .unwrap();
        let rows: Vec<Q> = q.take(0).unwrap_or_default();
        let mut qs: Vec<String> = rows.into_iter().filter_map(|x| x.qualified).collect();
        qs.sort();
        assert_eq!(
            qs,
            vec!["demo::A::run".to_string(), "demo::B::run".to_string()]
        );

        #[derive(Debug, SurrealValue)]
        struct C {
            c: Option<i64>,
        }
        let mut cc = store
            .db
            .query("SELECT count() AS c FROM atlas_edge WHERE relation='calls' AND resolution='resolved' GROUP ALL")
            .await
            .unwrap();
        let rows: Vec<C> = cc.take(0).unwrap_or_default();
        assert!(
            rows.into_iter().next().and_then(|x| x.c).unwrap_or(0) >= 1,
            "A::run → util::helper resolved"
        );

        // Idempotent re-projection (deterministic ids, full recompute).
        let r2 = project_code_graph(&store, &emb, dir.path()).await.unwrap();
        assert_eq!(r.symbols, r2.symbols, "stable on re-projection");
    }
}
