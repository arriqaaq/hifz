//! Memory ↔ code cross-linking.
//!
//! Two flows:
//! 1. **Auto-extract** — `auto_link_memory` runs from `enrich::save_enriched`.
//!    It scans the memory's title + content + content_long with `FILE_LINE_RE`
//!    and `FILE_PERMALINK_RE` (chunk-level) and `QUALIFIED_SYMBOL_RE`
//!    (symbol-level — M4 wires this on). Resolved hits become edges. By
//!    design (G9), bareword identifiers do NOT auto-link.
//! 2. **Explicit** — `link_memory_to_lines` and (M4) `link_memory_to_symbol`
//!    are called from `hifz_link_code` / `hifz_link_symbol` MCP tools.
//!
//! Re-anchoring across edits lives in `re_anchor_references`. It's invoked by
//! `code::index::index_walked` BEFORE deleting old chunks for a file: stored
//! `(ref_path, ref_start, ref_end)` in edge `metadata` lets us remap edges
//! to whichever new chunk overlaps the original line range.

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::link;

/// `path/to/file.ext:NN[-MM]` — the workhorse pattern in plan/decision text.
pub static FILE_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xi)
        (?P<path>[A-Za-z0-9_\-./]+\.(?:rs|py|pyi|ts|tsx|js|jsx|mjs|cjs|go|java|kt|kts|c|cpp|cc|cxx|h|hpp|hh|hxx))
        :(?P<start>\d+)(?:-(?P<end>\d+))?
        ",
    )
    .expect("FILE_LINE_RE compile")
});

/// GitHub permalink form: `…/blob/<sha>/path/to/file.rs#L42-L58`.
pub static FILE_PERMALINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xi)
        /blob/[0-9a-f]{7,40}/
        (?P<path>[A-Za-z0-9_\-./]+)
        \#L(?P<start>\d+)(?:-L(?P<end>\d+))?
        ",
    )
    .expect("FILE_PERMALINK_RE compile")
});

/// Qualified symbol form: `module::name`, `Type::method`, `path/file.rs::name`.
/// **Only** qualified patterns auto-link to symbols (G9). Bareword identifiers
/// require the explicit `hifz_link_symbol` tool.
pub static QUALIFIED_SYMBOL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xi)\b
        (?P<qual>
            (?:[A-Za-z0-9_\-./]+\.(?:rs|py|pyi|ts|tsx|js|jsx|mjs|cjs|go|java|kt|kts|c|cpp|cc|cxx|h|hpp|hh|hxx)::)?
            [A-Za-z_][A-Za-z0-9_]*
            (?:::[A-Za-z_][A-Za-z0-9_]*)+
        )\b
        ",
    )
    .expect("QUALIFIED_SYMBOL_RE compile")
});

#[derive(Debug, Default, serde::Serialize)]
pub struct AutoLinkReport {
    pub edges_created: usize,
    pub unresolved_paths: Vec<String>,
    pub unresolved_symbols: Vec<String>,
}

/// Auto-extract code references from `texts` and create edges from `memory_id`.
/// Errors per-match are logged + swallowed; the function never aborts the
/// outer save pipeline.
pub async fn auto_link_memory(
    db: &Surreal<Db>,
    memory_id: &RecordId,
    project: &str,
    texts: &[&str],
) -> Result<AutoLinkReport> {
    let mut report = AutoLinkReport::default();
    let mut seen_chunk_pairs: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for text in texts {
        if text.is_empty() {
            continue;
        }
        // Plain `path:NN[-MM]` form.
        for cap in FILE_LINE_RE.captures_iter(text) {
            let path = &cap["path"];
            let start: usize = cap["start"].parse().unwrap_or(0);
            let end: Option<usize> = cap.name("end").and_then(|m| m.as_str().parse().ok());
            apply_chunk_link(
                db,
                memory_id,
                project,
                path,
                start,
                end,
                "text_match",
                Some("auto-extracted from text"),
                &mut report,
                &mut seen_chunk_pairs,
            )
            .await;
        }
        // GitHub permalink form.
        for cap in FILE_PERMALINK_RE.captures_iter(text) {
            let path = &cap["path"];
            let start: usize = cap["start"].parse().unwrap_or(0);
            let end: Option<usize> = cap.name("end").and_then(|m| m.as_str().parse().ok());
            apply_chunk_link(
                db,
                memory_id,
                project,
                path,
                start,
                end,
                "text_match",
                Some("auto-extracted from permalink"),
                &mut report,
                &mut seen_chunk_pairs,
            )
            .await;
        }
        // Qualified symbol form (M4).
        for cap in QUALIFIED_SYMBOL_RE.captures_iter(text) {
            let qual = &cap["qual"];
            apply_symbol_link(
                db,
                memory_id,
                project,
                qual,
                "text_match",
                Some("auto-extracted qualified symbol"),
                &mut report,
            )
            .await;
        }
    }

    Ok(report)
}

#[allow(clippy::too_many_arguments)]
async fn apply_chunk_link(
    db: &Surreal<Db>,
    memory_id: &RecordId,
    project: &str,
    path: &str,
    start: usize,
    end: Option<usize>,
    via: &str,
    reason: Option<&str>,
    report: &mut AutoLinkReport,
    seen: &mut std::collections::HashSet<(String, String)>,
) {
    let end = end.unwrap_or(start);
    if start == 0 {
        return;
    }
    let chunks = match resolve_chunks(db, project, path, start, end).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("resolve_chunks failed for {path}:{start}-{end}: {e}");
            return;
        }
    };
    if chunks.is_empty() {
        report
            .unresolved_paths
            .push(format!("{path}:{start}-{end}"));
        return;
    }

    let mem_key = format!("{memory_id:?}");
    for c in chunks {
        let chunk_key = format!("{:?}", c.id);
        if !seen.insert((mem_key.clone(), chunk_key)) {
            continue;
        }
        if let Err(e) =
            link::upsert_edge(db, memory_id, &c.id, "references", via, 0.9, reason).await
        {
            tracing::warn!("upsert references edge failed: {e}");
            continue;
        }
        // Touch the chunk so cold-decay (M5) sees it as recently used.
        let _ = db
            .query("UPDATE type::record($id) SET last_referenced_at = time::now()")
            .bind(("id", c.id.clone()))
            .await;
        // Annotate the edge with the original anchor metadata. Looked up so
        // re_anchor_references can remap on file change.
        let _ = db
            .query(
                "UPDATE edge SET metadata = { \
                   ref_path: $path, ref_start: $rs, ref_end: $re, \
                   matched_chunk_lines: $lines, anchor_version: 1 } \
                 WHERE in = $from AND out = $to AND relation = 'references'",
            )
            .bind(("from", memory_id.clone()))
            .bind(("to", c.id.clone()))
            .bind(("path", path.to_string()))
            .bind(("rs", start as i64))
            .bind(("re", end as i64))
            .bind((
                "lines",
                format!("{}-{}", c.start_line.unwrap_or(0), c.end_line.unwrap_or(0)),
            ))
            .await;
        report.edges_created += 1;
    }
}

async fn apply_symbol_link(
    db: &Surreal<Db>,
    memory_id: &RecordId,
    project: &str,
    qualified: &str,
    via: &str,
    reason: Option<&str>,
    report: &mut AutoLinkReport,
) {
    let symbols = match resolve_symbols(db, project, qualified, None, None).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("resolve_symbols failed for {qualified}: {e}");
            return;
        }
    };
    if symbols.is_empty() {
        report.unresolved_symbols.push(qualified.to_string());
        return;
    }
    if symbols.len() > 1 {
        // Ambiguity: multiple symbols share this qualified name. Drop with a
        // log entry rather than guessing — G9 says ambiguity → unresolved.
        report.unresolved_symbols.push(format!(
            "{qualified} (ambiguous: {} matches)",
            symbols.len()
        ));
        return;
    }
    let s = &symbols[0];
    if let Err(e) =
        link::upsert_edge(db, memory_id, &s.id, "references_symbol", via, 0.9, reason).await
    {
        tracing::warn!("upsert references_symbol edge failed: {e}");
        return;
    }
    // No `matched_symbol`/`anchor_version` metadata (E4): the symbol's
    // record id is now deterministic and stable across reindex, so the
    // edge needs no re-anchor breadcrumb.
    report.edges_created += 1;
}

#[derive(Debug, SurrealValue)]
struct ChunkRow {
    id: RecordId,
    start_line: Option<i64>,
    end_line: Option<i64>,
}

#[derive(Debug, SurrealValue)]
struct SymbolRow {
    id: RecordId,
}

async fn resolve_chunks(
    db: &Surreal<Db>,
    project: &str,
    path: &str,
    start: usize,
    end: usize,
) -> Result<Vec<ChunkRow>> {
    let mut resp = db
        .query(
            "SELECT id, start_line, end_line FROM code_chunk \
             WHERE project = $p AND path = $path \
               AND start_line <= $end AND end_line >= $start",
        )
        .bind(("p", project.to_string()))
        .bind(("path", path.to_string()))
        .bind(("start", start as i64))
        .bind(("end", end as i64))
        .await?;
    Ok(resp.take(0).unwrap_or_default())
}

async fn resolve_symbols(
    db: &Surreal<Db>,
    project: &str,
    qualified: &str,
    kind: Option<&str>,
    file: Option<&str>,
) -> Result<Vec<SymbolRow>> {
    // We accept matches on `qualified` OR `name` — many auto-extracts arrive
    // as `Type::method` while the indexer (M1) stores `qualified == name` for
    // simple cases. M4 will derive proper qualified paths.
    let mut sql = String::from(
        "SELECT id FROM code_symbol WHERE project = $p \
         AND (qualified = $q OR name = $q)",
    );
    if kind.is_some() {
        sql.push_str(" AND kind = $kind");
    }
    if file.is_some() {
        sql.push_str(" AND path = $file");
    }
    let mut q = db
        .query(&sql)
        .bind(("p", project.to_string()))
        .bind(("q", qualified.to_string()));
    if let Some(k) = kind {
        q = q.bind(("kind", k.to_string()));
    }
    if let Some(f) = file {
        q = q.bind(("file", f.to_string()));
    }
    let mut resp = q.await?;
    Ok(resp.take(0).unwrap_or_default())
}

/// Explicit chunk linker — backs `hifz_link_code` MCP tool.
pub async fn link_memory_to_lines(
    db: &Surreal<Db>,
    memory_id: &RecordId,
    project: &str,
    file: &str,
    start_line: usize,
    end_line: Option<usize>,
    reason: Option<&str>,
) -> Result<Vec<RecordId>> {
    let end_line = end_line.unwrap_or(start_line);
    let chunks = resolve_chunks(db, project, file, start_line, end_line).await?;
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    let mut linked = Vec::with_capacity(chunks.len());
    for c in chunks {
        let _ = link::upsert_edge(
            db,
            memory_id,
            &c.id,
            "references",
            "reference",
            0.95,
            reason.or(Some("explicit link via hifz_link_code")),
        )
        .await;
        let _ = db
            .query(
                "UPDATE edge SET metadata = { \
                   ref_path: $path, ref_start: $rs, ref_end: $re, \
                   matched_chunk_lines: $lines, anchor_version: 1 } \
                 WHERE in = $from AND out = $to AND relation = 'references'",
            )
            .bind(("from", memory_id.clone()))
            .bind(("to", c.id.clone()))
            .bind(("path", file.to_string()))
            .bind(("rs", start_line as i64))
            .bind(("re", end_line as i64))
            .bind((
                "lines",
                format!("{}-{}", c.start_line.unwrap_or(0), c.end_line.unwrap_or(0)),
            ))
            .await;
        let _ = db
            .query("UPDATE type::record($id) SET last_referenced_at = time::now()")
            .bind(("id", c.id.clone()))
            .await;
        linked.push(c.id);
    }
    Ok(linked)
}

/// Explicit symbol linker — backs `hifz_link_symbol`.
pub async fn link_memory_to_symbol(
    db: &Surreal<Db>,
    memory_id: &RecordId,
    project: &str,
    name: &str,
    kind: Option<&str>,
    file: Option<&str>,
    reason: Option<&str>,
) -> Result<Vec<RecordId>> {
    let symbols = resolve_symbols(db, project, name, kind, file).await?;
    let mut linked = Vec::with_capacity(symbols.len());
    for s in symbols {
        let _ = link::upsert_edge(
            db,
            memory_id,
            &s.id,
            "references_symbol",
            "reference",
            0.95,
            reason.or(Some("explicit link via hifz_link_symbol")),
        )
        .await;
        linked.push(s.id);
    }
    Ok(linked)
}

// ---------------------------------------------------------------------------
// Re-anchoring across edits (G6)
// ---------------------------------------------------------------------------

/// Snapshot of an edge's anchor metadata before its target is deleted.
#[derive(Debug, Clone, SurrealValue)]
pub struct ArchivedRef {
    pub edge_id: RecordId,
    pub from: RecordId,
    pub ref_path: Option<String>,
    pub ref_start: Option<i64>,
    pub ref_end: Option<i64>,
    pub anchor_version: Option<i64>,
}

/// Capture all `references` edges whose `out` endpoint is a chunk of `file_id`.
/// Called by `index_walked` BEFORE deleting old chunks. The returned snapshots
/// feed `re_anchor_references` after new chunks are written.
pub async fn snapshot_references(db: &Surreal<Db>, file_id: &RecordId) -> Result<Vec<ArchivedRef>> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        edge_id: RecordId,
        from: RecordId,
        ref_path: Option<String>,
        ref_start: Option<i64>,
        ref_end: Option<i64>,
        anchor_version: Option<i64>,
    }
    let mut resp = db
        .query(
            "SELECT id AS edge_id, in AS from, \
                    metadata.ref_path AS ref_path, \
                    metadata.ref_start AS ref_start, \
                    metadata.ref_end AS ref_end, \
                    metadata.anchor_version AS anchor_version \
             FROM edge \
             WHERE relation = 'references' \
               AND out IN (SELECT VALUE id FROM code_chunk WHERE file = $fid)",
        )
        .bind(("fid", file_id.clone()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|r| ArchivedRef {
            edge_id: r.edge_id,
            from: r.from,
            ref_path: r.ref_path,
            ref_start: r.ref_start,
            ref_end: r.ref_end,
            anchor_version: r.anchor_version,
        })
        .collect())
}

// `snapshot_symbol_references` / `re_anchor_symbol_references` removed in
// E4. Symbol identity is now a deterministic `(project,qualified)` id
// (`codeintel`), so `references_symbol` edges survive reindex by
// construction; symbol renames are reconciled structurally there. Chunk
// re-anchoring (`snapshot_references`/`re_anchor_references`) is unchanged.

/// After new chunks are written, retarget archived edges to whichever new
/// chunk overlaps the original line range. If none overlap, mark the edge
/// dropped (`metadata.dropped_reason='lines_deleted'`) and DELETE.
pub async fn re_anchor_references(
    db: &Surreal<Db>,
    project: &str,
    archived: &[ArchivedRef],
) -> Result<usize> {
    let mut rewritten = 0usize;
    for a in archived {
        let (Some(path), Some(start), Some(end)) = (a.ref_path.as_deref(), a.ref_start, a.ref_end)
        else {
            // Missing anchor metadata — drop the edge as orphan.
            let _ = db
                .query("DELETE type::record($id)")
                .bind(("id", a.edge_id.clone()))
                .await;
            continue;
        };
        let chunks = resolve_chunks(
            db,
            project,
            path,
            start.max(0) as usize,
            end.max(0) as usize,
        )
        .await?;
        if chunks.is_empty() {
            // The lines vanished — record the reason and drop.
            let now = chrono::Utc::now().to_rfc3339();
            let _ = db
                .query(
                    "UPDATE type::record($id) SET \
                     metadata.dropped_at = $now, \
                     metadata.dropped_reason = 'lines_deleted'",
                )
                .bind(("id", a.edge_id.clone()))
                .bind(("now", now))
                .await;
            let _ = db
                .query("DELETE type::record($id)")
                .bind(("id", a.edge_id.clone()))
                .await;
            continue;
        }
        // Best-overlap chunk: simply pick the first match. Re-anchor the
        // existing edge instead of recreating it (preserves edge id, score).
        let target = &chunks[0];
        let new_version = a.anchor_version.unwrap_or(0) + 1;
        let _ = db
            .query(
                "UPDATE type::record($id) SET out = $new_out, \
                 metadata.matched_chunk_lines = $lines, \
                 metadata.anchor_version = $ver",
            )
            .bind(("id", a.edge_id.clone()))
            .bind(("new_out", target.id.clone()))
            .bind((
                "lines",
                format!(
                    "{}-{}",
                    target.start_line.unwrap_or(0),
                    target.end_line.unwrap_or(0)
                ),
            ))
            .bind(("ver", new_version))
            .await;
        rewritten += 1;
    }
    Ok(rewritten)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_line_re_basic() {
        let m = FILE_LINE_RE
            .captures("see src/db.rs:43-50 for details")
            .unwrap();
        assert_eq!(&m["path"], "src/db.rs");
        assert_eq!(&m["start"], "43");
        assert_eq!(m.name("end").unwrap().as_str(), "50");
    }

    #[test]
    fn file_line_re_single_line() {
        let m = FILE_LINE_RE
            .captures("look at src/foo.py:7 right here")
            .unwrap();
        assert_eq!(&m["path"], "src/foo.py");
        assert_eq!(&m["start"], "7");
        assert!(m.name("end").is_none());
    }

    #[test]
    fn permalink_re_extracts_sha_path_lines() {
        let url = "https://github.com/me/proj/blob/abc1234/src/main.rs#L10-L20";
        let m = FILE_PERMALINK_RE.captures(url).unwrap();
        assert_eq!(&m["path"], "src/main.rs");
        assert_eq!(&m["start"], "10");
        assert_eq!(m.name("end").unwrap().as_str(), "20");
    }

    #[test]
    fn qualified_symbol_re_requires_double_colon() {
        // Bareword identifier — must NOT match (G9).
        assert!(
            QUALIFIED_SYMBOL_RE
                .captures("the parse_chunk function")
                .is_none()
        );
        // Qualified — matches.
        let m = QUALIFIED_SYMBOL_RE
            .captures("see chunk::persist_chunks here")
            .unwrap();
        assert_eq!(&m["qual"], "chunk::persist_chunks");
    }
}
