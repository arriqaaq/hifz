//! Project-wide code-intelligence pass (E4).
//!
//! Replaces the old per-file `.scm` symbol writer. Walks every supported
//! file, builds each file's `FileGraph` via the hifz-core code-intel core
//! (semantic scope-qualified identity), resolves references project-wide
//! (`resolve` — 3-state, nothing dropped), then **UPSERTs** `code_symbol`
//! on a deterministic `(project,qualified)` id (no wipe-recreate, so
//! `references_symbol` edges survive reindex by construction) and rebuilds
//! the derived `calls`/`imports`/`contains` graph with `resolution`.
//!
//! Symbol renames are reconciled structurally (identical `body_hash`,
//! different `qualified`) → inbound `references_symbol` migrated. This
//! replaces the deleted `matched_symbol` string re-anchor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::code::lang::Language;
use crate::code::walker::{WalkOpts, walk};
use crate::db::Db;
use kernel::code_parse::graph::{FileGraph, walk_file};
use kernel::code_parse::langmod::module_path;
use kernel::code_parse::resolve::{Resolution, resolve_project};

/// A parsed file held in the live-watcher's in-memory cache: the tree-sitter
/// `FileGraph` plus the content hash that produced it (to skip no-op events)
/// and the metadata `resolve_and_persist` needs to anchor symbols.
#[derive(Debug, Clone)]
pub struct CachedFile {
    pub abs: PathBuf,
    pub lang: Language,
    pub content_hash: String,
    pub graph: FileGraph,
}

/// Per-project map `rel_path -> CachedFile`. The watcher re-parses only the
/// changed file and re-runs the (pure, cheap) `resolve_project` over this set,
/// so the symbol/call graph stays consistent without re-parsing the repo.
pub type FileGraphCache = HashMap<String, CachedFile>;

/// Parse one file into a `CachedFile` (read + hash + tree-sitter walk). Pure —
/// no DB. Returns `None` for unreadable, non-UTF8, or unsupported-language files.
pub fn build_file_graph(abs: &Path, rel: &str, root: &Path) -> Option<CachedFile> {
    let bytes = std::fs::read(abs).ok()?;
    let source = std::str::from_utf8(&bytes).ok()?;
    let lang = Language::from_path(abs).unwrap_or(Language::Plain);
    if lang == Language::Plain {
        return None;
    }
    let mp = module_path(lang, abs, root);
    let graph = walk_file(lang, source, &mp).ok()?;
    let _ = rel;
    Some(CachedFile {
        abs: abs.to_path_buf(),
        lang,
        content_hash: sha256_hex(&bytes),
        graph,
    })
}

/// Walk `root` (gitignore-honest) and parse every supported file into a
/// `FileGraphCache`. Pure — no DB. Used to warm the watcher's cache.
pub fn build_cache(root: &Path) -> FileGraphCache {
    let mut cache = FileGraphCache::new();
    let files = match walk(root, &WalkOpts::default()) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("intel::build_cache walk failed: {e}");
            return cache;
        }
    };
    for f in files {
        if let Some(cf) = build_file_graph(&f.abs, &f.rel, root) {
            cache.insert(f.rel, cf);
        }
    }
    cache
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

#[derive(Debug, Default, serde::Serialize)]
pub struct CodeGraphReport {
    pub symbols: usize,
    pub edges: usize,
    pub externals: usize,
    pub renamed: usize,
    pub deleted: usize,
}

fn key(project: &str, qualified: &str) -> String {
    let mut h = Sha256::new();
    h.update(project.as_bytes());
    h.update([0u8]);
    h.update(qualified.as_bytes());
    hex::encode(&h.finalize()[..16])
}

#[derive(Debug, SurrealValue)]
struct IdRow {
    id: Option<RecordId>,
}

#[derive(Debug, SurrealValue)]
struct PriorSym {
    id: Option<RecordId>,
    qualified: Option<String>,
    body_hash: Option<String>,
}

/// Ensure a `code_file` row exists for `rel`; return its id. (index_repo's
/// chunk pass creates these for chunked files; this covers zero-chunk or
/// skipped files so every symbol still anchors — nothing dropped.)
async fn ensure_file(
    db: &Surreal<Db>,
    project: &str,
    rel: &str,
    abs: &str,
    lang: Language,
) -> Result<RecordId> {
    let mut resp = db
        .query("SELECT id FROM code_file WHERE project = $p AND path = $r LIMIT 1")
        .bind(("p", project.to_string()))
        .bind(("r", rel.to_string()))
        .await?;
    let rows: Vec<IdRow> = resp.take(0).unwrap_or_default();
    if let Some(id) = rows.into_iter().next().and_then(|r| r.id) {
        return Ok(id);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut resp = db
        .query(
            "CREATE code_file SET project=$p, path=$r, abs_path=$abs, language=$lang, \
             size_bytes=0, mtime_ns=0, content_hash='', chunk_count=0, indexed_at=$now RETURN id",
        )
        .bind(("p", project.to_string()))
        .bind(("r", rel.to_string()))
        .bind(("abs", abs.to_string()))
        .bind(("lang", lang.as_str().to_string()))
        .bind(("now", now))
        .await?;
    let rows: Vec<IdRow> = resp.take(0).unwrap_or_default();
    rows.into_iter()
        .next()
        .and_then(|r| r.id)
        .context("CREATE code_file returned no id")
}

pub async fn index_code_graph(
    db: &Surreal<Db>,
    project: &str,
    root: &Path,
) -> Result<CodeGraphReport> {
    let cache = build_cache(root);
    resolve_and_persist(db, project, &cache, None).await
}

/// Resolve the project-wide symbol/call graph from an in-memory `FileGraphCache`
/// and persist it. `resolve_project` is pure and cheap, so this re-runs over the
/// whole cached set on every change — the cost the watcher avoids is re-parsing
/// files, not this resolve. When `changed` is `Some`, only those files' symbols
/// are UPSERTed (unchanged files' defs are byte-identical, so their writes and
/// per-symbol `chunk_span` queries are skipped); rename/stale reconciliation and
/// the derived-edge rebuild always run project-wide for correctness.
pub async fn resolve_and_persist(
    db: &Surreal<Db>,
    project: &str,
    cache: &FileGraphCache,
    changed: Option<&HashSet<String>>,
) -> Result<CodeGraphReport> {
    // Resolve existing code_file ids in one query; ensure rows for new files.
    #[derive(Debug, SurrealValue)]
    struct FileRow {
        id: RecordId,
        path: Option<String>,
    }
    let mut existing: HashMap<String, RecordId> = HashMap::new();
    {
        let mut resp = db
            .query("SELECT id, path FROM code_file WHERE project = $p AND deleted_at IS NONE")
            .bind(("p", project.to_string()))
            .await?;
        let rows: Vec<FileRow> = resp.take(0).unwrap_or_default();
        for r in rows {
            if let Some(p) = r.path {
                existing.insert(p, r.id);
            }
        }
    }

    // Build association maps from the cache (no file IO, no re-parse).
    let mut graphs: Vec<FileGraph> = Vec::with_capacity(cache.len());
    let mut qual2file: HashMap<String, (RecordId, String, Language)> = HashMap::new();
    let mut mod2file: HashMap<String, RecordId> = HashMap::new();
    for (rel, cf) in cache {
        let file_id = match existing.get(rel) {
            Some(id) => id.clone(),
            None => ensure_file(db, project, rel, &cf.abs.to_string_lossy(), cf.lang).await?,
        };
        mod2file.insert(cf.graph.module_path.clone(), file_id.clone());
        for d in &cf.graph.defs {
            qual2file.insert(d.qualified.clone(), (file_id.clone(), rel.clone(), cf.lang));
        }
        graphs.push(cf.graph.clone());
    }

    let pg = resolve_project(graphs);
    let now = chrono::Utc::now().to_rfc3339();
    let mut report = CodeGraphReport::default();

    // Snapshot prior symbols (rename reconciliation + stale delete)
    let mut resp = db
        .query("SELECT id, qualified, body_hash FROM code_symbol WHERE project = $p")
        .bind(("p", project.to_string()))
        .await?;
    let prior: Vec<PriorSym> = resp.take(0).unwrap_or_default();
    let new_quals: std::collections::HashSet<&str> =
        pg.defs.iter().map(|d| d.qualified.as_str()).collect();
    // body_hash → all new qualifieds with that body. Rename is only
    // reconciled on a UNIQUE match (empty/trivial bodies collide — never
    // mis-migrate; an ambiguous case is left unreconciled, not guessed).
    let mut new_by_bodyhash: HashMap<&str, Vec<&str>> = HashMap::new();
    for d in &pg.defs {
        new_by_bodyhash
            .entry(d.body_hash.as_str())
            .or_default()
            .push(d.qualified.as_str());
    }

    // UPSERT every symbol on its deterministic id. In incremental mode only
    // changed files' symbols are rewritten — unchanged files' defs are
    // byte-identical, so their rows (and chunk_span queries) are untouched.
    for d in &pg.defs {
        let Some((file_id, rel, lang)) = qual2file.get(&d.qualified) else {
            continue;
        };
        if let Some(changed) = changed
            && !changed.contains(rel)
        {
            continue;
        }
        let id = key(project, &d.qualified);
        let parent = d
            .parent
            .as_ref()
            .map(|p| ("code_symbol".to_string(), key(project, p)));
        // chunk_span: every chunk this symbol's line range overlaps.
        let mut cs = db
            .query(
                "SELECT VALUE id FROM code_chunk \
                 WHERE file = $fid AND start_line <= $el AND end_line >= $sl",
            )
            .bind(("fid", file_id.clone()))
            .bind(("sl", d.start_line as i64))
            .bind(("el", d.end_line as i64))
            .await?;
        let chunk_span: Vec<RecordId> = cs.take(0).unwrap_or_default();

        db.query(
            "UPSERT type::record($rid) SET \
             project=$p, name=$n, qualified=$q, kind=$k, language=$lang, \
             file=$fid, path=$path, start_line=$sl, end_line=$el, \
             start_byte=$sb, end_byte=$eb, signature=$sig, doc=$doc, body_hash=$bh, \
             parent_symbol=$parent, chunk_span=$cs, created_at=$now",
        )
        .bind(("rid", format!("code_symbol:{id}")))
        .bind(("p", project.to_string()))
        .bind(("n", d.name.clone()))
        .bind(("q", d.qualified.clone()))
        .bind(("k", d.kind.clone()))
        .bind(("lang", lang.as_str().to_string()))
        .bind(("fid", file_id.clone()))
        .bind(("path", rel.clone()))
        .bind(("sl", d.start_line as i64))
        .bind(("el", d.end_line as i64))
        .bind(("sb", d.start_byte as i64))
        .bind(("eb", d.end_byte as i64))
        .bind(("sig", d.signature.clone()))
        .bind(("doc", d.doc.clone()))
        .bind(("bh", d.body_hash.clone()))
        .bind((
            "parent",
            parent.map(|(tb, k)| surrealdb::types::RecordId::new(tb, k)),
        ))
        .bind(("cs", chunk_span))
        .bind(("now", now.clone()))
        .await?
        .check()?;
        // Structural containment: symbol --part_of--> code_file.
        let srid = RecordId::new("code_symbol", key(project, &d.qualified));
        let _ = db
            .query(
                "RELATE $srid->edge->$fid \
                 SET relation='part_of', via='code', score=1.0, \
                 resolution='resolved', created_at=$now",
            )
            .bind(("srid", srid))
            .bind(("fid", file_id.clone()))
            .bind(("now", now.clone()))
            .await;
        report.symbols += 1;
    }

    // Rename reconciliation + stale delete
    for ps in &prior {
        let (Some(old_id), Some(oq)) = (ps.id.clone(), ps.qualified.as_deref()) else {
            continue;
        };
        if new_quals.contains(oq) {
            continue; // still present (UPSERT refreshed it)
        }
        // qualified vanished. Same body in exactly one new symbol →
        // structural rename (unique match only — never guess).
        if let Some(bh) = ps.body_hash.as_deref()
            && let Some(cands) = new_by_bodyhash.get(bh).filter(|v| v.len() == 1)
        {
            let new_q = cands[0];
            let new_id = surrealdb::types::RecordId::new("code_symbol", key(project, new_q));
            // A relation edge's `out` endpoint is immutable, so re-RELATE
            // each inbound memory→symbol edge onto the renamed target
            // (preserving via/score/reason), then the stale-delete below
            // removes the old symbol + its now-superseded edges.
            #[derive(Debug, SurrealValue)]
            struct Inb {
                r#in: RecordId,
                via: Option<String>,
                score: Option<f64>,
                reason: Option<String>,
            }
            let mut ir = db
                .query(
                    "SELECT in, via, score, reason FROM edge \
                         WHERE relation = 'references_symbol' AND out = $old",
                )
                .bind(("old", old_id.clone()))
                .await?;
            let inbound: Vec<Inb> = ir.take(0).unwrap_or_default();
            for ib in inbound {
                let _ = db
                    .query(
                        "RELATE $m->edge->$new SET relation='references_symbol', \
                             via=$via, score=$score, reason=$reason, created_at=$now",
                    )
                    .bind(("m", ib.r#in))
                    .bind(("new", new_id.clone()))
                    .bind(("via", ib.via.unwrap_or_else(|| "rename".into())))
                    .bind(("score", ib.score.unwrap_or(0.9)))
                    .bind(("reason", ib.reason))
                    .bind(("now", now.clone()))
                    .await;
            }
            report.renamed += 1;
        }
        let _ = db
            .query("DELETE edge WHERE in = $old OR out = $old")
            .bind(("old", old_id.clone()))
            .await;
        let _ = db
            .query("DELETE type::record($old)")
            .bind(("old", old_id))
            .await;
        report.deleted += 1;
    }

    // Rebuild derived code edges (calls/imports/contains/part_of)
    // Fully recomputed each run → idempotent. references_symbol (memory→
    // code) is NOT in this set, so it is preserved across reindex.
    let _ = db
        .query(
            "DELETE edge WHERE via = 'code' AND relation IN \
             ['calls','imports','contains','part_of'] \
             AND in IN (SELECT VALUE id FROM code_symbol WHERE project = $p)",
        )
        .bind(("p", project.to_string()))
        .await;
    let _ = db
        .query(
            "DELETE edge WHERE via = 'code' AND relation IN ['imports'] \
             AND in IN (SELECT VALUE id FROM code_file WHERE project = $p)",
        )
        .bind(("p", project.to_string()))
        .await;

    for e in &pg.edges {
        // Resolve endpoints to record ids.
        let from = if e.relation == "imports" {
            // module → file
            mod2file
                .iter()
                .find(|(m, _)| e.from == **m)
                .map(|(_, fid)| fid.clone())
                .or_else(|| qual2file.get(&e.from).map(|(fid, _, _)| fid.clone()))
        } else if qual2file.contains_key(&e.from) {
            Some(RecordId::new("code_symbol", key(project, &e.from)))
        } else {
            None
        };
        let Some(from) = from else { continue };

        // Targets: Resolved→symbol; External→external_symbol; Ambiguous→
        // fan out to each known candidate (uncertainty represented, never
        // a guessed single edge, never dropped).
        let targets: Vec<(RecordId, f64)> = match e.resolution {
            Resolution::Resolved => {
                if qual2file.contains_key(&e.to) {
                    vec![(RecordId::new("code_symbol", key(project, &e.to)), 1.0)]
                } else {
                    vec![]
                }
            }
            Resolution::External => {
                let ek = key(project, &e.to);
                db.query(
                    "UPSERT type::record($rid) SET \
                     project=$p, canonical=$c, language='', created_at=$now",
                )
                .bind(("rid", format!("external_symbol:{ek}")))
                .bind(("p", project.to_string()))
                .bind(("c", e.to.clone()))
                .bind(("now", now.clone()))
                .await?
                .check()?;
                report.externals += 1;
                vec![(RecordId::new("external_symbol", ek), 1.0)]
            }
            Resolution::Ambiguous => {
                let known: Vec<&String> = e
                    .candidates
                    .iter()
                    .filter(|c| qual2file.contains_key(*c))
                    .collect();
                let n = known.len().max(1) as f64;
                known
                    .into_iter()
                    .map(|c| (RecordId::new("code_symbol", key(project, c)), 1.0 / n))
                    .collect()
            }
        };

        for (to, score) in targets {
            let _ = db
                .query(
                    "RELATE $f->edge->$t SET relation=$rel, via='code', \
                     score=$s, resolution=$res, created_at=$now, \
                     reason=$reason, metadata=$meta",
                )
                .bind(("f", from.clone()))
                .bind(("t", to))
                .bind(("rel", e.relation.to_string()))
                .bind(("s", score))
                .bind(("res", e.resolution.as_str().to_string()))
                .bind(("now", now.clone()))
                .bind(("reason", format!("code-intel {}", e.relation)))
                .bind((
                    "meta",
                    if e.candidates.is_empty() {
                        None
                    } else {
                        Some(serde_json::json!({ "candidates": e.candidates }))
                    },
                ))
                .await;
            report.edges += 1;
        }
    }

    tracing::info!(
        "intel: project={project} symbols={} edges={} ext={} renamed={} deleted={}",
        report.symbols,
        report.edges,
        report.externals,
        report.renamed,
        report.deleted
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_repo(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hifz_intel_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (rel, body) in files {
            let p = d.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        d
    }

    #[test]
    fn build_cache_parses_only_supported_sources() {
        let root = tmp_repo(
            "buildcache",
            &[
                ("Cargo.toml", "[package]\nname = \"demo\"\n"),
                ("src/lib.rs", "pub fn alpha() {}\npub struct Bar;\n"),
                ("notes.txt", "not source"),
            ],
        );
        let cache = build_cache(&root);

        // The Rust source is parsed into the cache; non-source files are not.
        assert!(
            cache.contains_key("src/lib.rs"),
            "got keys: {:?}",
            cache.keys().collect::<Vec<_>>()
        );
        assert!(!cache.contains_key("Cargo.toml"));
        assert!(!cache.contains_key("notes.txt"));

        // Its FileGraph carries the parsed definitions and a content hash.
        let cf = &cache["src/lib.rs"];
        assert!(cf.graph.defs.iter().any(|d| d.name == "alpha"));
        assert!(cf.graph.defs.iter().any(|d| d.name == "Bar"));
        assert!(!cf.content_hash.is_empty());
    }
}
