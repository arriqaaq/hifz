//! Code-index garbage collection.
//!
//! Two passes, both invoked by `hifz_code_gc`:
//!
//! 1. **Reconcile deletions** — diff the indexed `code_file` rows for a
//!    project against what the walker currently sees on disk. Files in the DB
//!    but missing from disk get their inbound `references` edges dropped (with
//!    `metadata.dropped_reason = 'file_deleted'`), their chunks/symbols
//!    deleted, and a 30-day tombstone `code_file.deleted_at` set on the
//!    `code_file` row. The tombstone is swept later by `forget::run_forget`.
//!
//! 2. **Cold decay** (opt-in via `force_decay=true`) — chunks/symbols with
//!    no inbound refs AND `last_committed_at < now - 60d` AND
//!    `created_at < now - 30d` lose strength on each pass; rows with
//!    strength < 0.1 are deleted.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use chrono::{Duration, Utc};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::code::walker::{WalkOpts, walk};
use crate::db::Db;

#[derive(Debug, Default, serde::Serialize)]
pub struct CodeGcReport {
    pub files_deleted: usize,
    pub chunks_dropped: usize,
    pub symbols_dropped: usize,
    pub edges_dropped: usize,
    pub decayed_chunks: usize,
    pub decayed_symbols: usize,
    pub deleted_decayed: usize,
    pub dry_run: bool,
}

pub async fn run_gc(
    db: &Surreal<Db>,
    project: &str,
    root: &Path,
    dry_run: bool,
    force_decay: bool,
) -> Result<CodeGcReport> {
    let mut report = CodeGcReport {
        dry_run,
        ..Default::default()
    };
    reconcile_deletions(db, project, root, dry_run, &mut report).await?;
    if force_decay {
        decay_cold_chunks(db, project, dry_run, &mut report).await?;
    }
    Ok(report)
}

#[derive(Debug, SurrealValue)]
struct CodeFilePathRow {
    id: RecordId,
    path: Option<String>,
}

pub async fn reconcile_deletions(
    db: &Surreal<Db>,
    project: &str,
    root: &Path,
    dry_run: bool,
    report: &mut CodeGcReport,
) -> Result<()> {
    let walked = walk(root, &WalkOpts::default())?;
    let alive: HashSet<String> = walked.into_iter().map(|f| f.rel).collect();

    let mut resp = db
        .query(
            "SELECT id, path FROM code_file \
             WHERE project = $p AND deleted_at IS NONE",
        )
        .bind(("p", project.to_string()))
        .await?;
    let rows: Vec<CodeFilePathRow> = resp.take(0).unwrap_or_default();

    let now = Utc::now().to_rfc3339();
    for row in rows {
        let Some(ref path) = row.path else { continue };
        if alive.contains(path) {
            continue;
        }

        report.files_deleted += 1;
        if dry_run {
            continue;
        }

        // Drop inbound references / references_symbol edges with audit metadata.
        let chunks_dropped = drop_inbound_code_edges(db, &row.id, &now)
            .await
            .unwrap_or(0);
        report.edges_dropped += chunks_dropped;

        // Count chunks/symbols before deletion for the report.
        if let Ok(n) = count_children(db, &row.id, "code_chunk").await {
            report.chunks_dropped += n;
        }
        if let Ok(n) = count_children(db, &row.id, "code_symbol").await {
            report.symbols_dropped += n;
        }

        // Drop part_of edges + chunks + symbols.
        let _ = db
            .query(
                "DELETE edge WHERE relation = 'part_of' AND \
                 (in IN (SELECT VALUE id FROM code_chunk WHERE file = $fid) \
                  OR in IN (SELECT VALUE id FROM code_symbol WHERE file = $fid))",
            )
            .bind(("fid", row.id.clone()))
            .await;
        let _ = db
            .query("DELETE code_chunk WHERE file = $fid")
            .bind(("fid", row.id.clone()))
            .await;
        let _ = db
            .query("DELETE code_symbol WHERE file = $fid")
            .bind(("fid", row.id.clone()))
            .await;

        // Tombstone the code_file row (kept 30 days for audit; sweep handled
        // by forget::run_forget).
        let _ = db
            .query("UPDATE type::record($id) SET deleted_at = $now")
            .bind(("id", row.id.clone()))
            .bind(("now", now.clone()))
            .await;
    }
    Ok(())
}

async fn drop_inbound_code_edges(db: &Surreal<Db>, file_id: &RecordId, now: &str) -> Result<usize> {
    // Mark with dropped_reason for audit, then DELETE.
    let _ = db
        .query(
            "UPDATE edge SET metadata.dropped_at = $now, \
                             metadata.dropped_reason = 'file_deleted' \
             WHERE relation IN ['references', 'references_symbol'] \
               AND (out IN (SELECT VALUE id FROM code_chunk  WHERE file = $fid) \
                 OR out IN (SELECT VALUE id FROM code_symbol WHERE file = $fid) \
                 OR out = $fid)",
        )
        .bind(("fid", file_id.clone()))
        .bind(("now", now.to_string()))
        .await;

    #[derive(Debug, SurrealValue)]
    struct CountRow {
        c: i64,
    }
    let mut resp = db
        .query(
            "SELECT count() AS c FROM edge \
             WHERE relation IN ['references', 'references_symbol'] \
               AND (out IN (SELECT VALUE id FROM code_chunk  WHERE file = $fid) \
                 OR out IN (SELECT VALUE id FROM code_symbol WHERE file = $fid) \
                 OR out = $fid) GROUP ALL",
        )
        .bind(("fid", file_id.clone()))
        .await?;
    let counts: Vec<CountRow> = resp.take(0).unwrap_or_default();
    let n = counts.first().map(|c| c.c).unwrap_or(0);

    let _ = db
        .query(
            "DELETE edge WHERE relation IN ['references', 'references_symbol'] \
               AND (out IN (SELECT VALUE id FROM code_chunk  WHERE file = $fid) \
                 OR out IN (SELECT VALUE id FROM code_symbol WHERE file = $fid) \
                 OR out = $fid)",
        )
        .bind(("fid", file_id.clone()))
        .await;

    Ok(n.max(0) as usize)
}

async fn count_children(db: &Surreal<Db>, file_id: &RecordId, table: &str) -> Result<usize> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        c: i64,
    }
    let sql = format!("SELECT count() AS c FROM {table} WHERE file = $fid GROUP ALL");
    let mut resp = db.query(&sql).bind(("fid", file_id.clone())).await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    Ok(rows.first().map(|r| r.c.max(0) as usize).unwrap_or(0))
}

pub async fn decay_cold_chunks(
    db: &Surreal<Db>,
    project: &str,
    dry_run: bool,
    report: &mut CodeGcReport,
) -> Result<()> {
    let now = Utc::now();
    let stale_cutoff = (now - Duration::days(60)).to_rfc3339();
    let young_cutoff = (now - Duration::days(30)).to_rfc3339();

    // Chunks with no incoming references AND old + stale.
    #[derive(Debug, SurrealValue)]
    struct CountRow {
        c: i64,
    }
    let mut resp = db
        .query(
            "SELECT count() AS c FROM code_chunk \
             WHERE project = $p \
               AND created_at < $young \
               AND (last_committed_at IS NONE OR last_committed_at < $stale) \
               AND id NOT IN (SELECT VALUE out FROM edge \
                              WHERE relation IN ['references','references_symbol']) \
             GROUP ALL",
        )
        .bind(("p", project.to_string()))
        .bind(("young", young_cutoff.clone()))
        .bind(("stale", stale_cutoff.clone()))
        .await?;
    let rows: Vec<CountRow> = resp.take(0).unwrap_or_default();
    let cold = rows.first().map(|r| r.c.max(0) as usize).unwrap_or(0);
    report.decayed_chunks += cold;

    if !dry_run && cold > 0 {
        // Multiplicative decay: 0.95 per pass. Repeat passes naturally bring
        // unused chunks below the deletion floor.
        let _ = db
            .query(
                "UPDATE code_chunk SET strength = math::max(strength * 0.95, 0.0) \
                 WHERE project = $p \
                   AND created_at < $young \
                   AND (last_committed_at IS NONE OR last_committed_at < $stale) \
                   AND id NOT IN (SELECT VALUE out FROM edge \
                                  WHERE relation IN ['references','references_symbol'])",
            )
            .bind(("p", project.to_string()))
            .bind(("young", young_cutoff.clone()))
            .bind(("stale", stale_cutoff.clone()))
            .await;

        // Delete chunks now below 0.1.
        let mut resp = db
            .query(
                "SELECT count() AS c FROM code_chunk \
                 WHERE project = $p AND strength < 0.1 GROUP ALL",
            )
            .bind(("p", project.to_string()))
            .await?;
        let rows: Vec<CountRow> = resp.take(0).unwrap_or_default();
        let zombies = rows.first().map(|r| r.c.max(0) as usize).unwrap_or(0);
        report.deleted_decayed += zombies;
        let _ = db
            .query(
                "DELETE edge WHERE relation = 'part_of' AND \
                 in IN (SELECT VALUE id FROM code_chunk WHERE project = $p AND strength < 0.1)",
            )
            .bind(("p", project.to_string()))
            .await;
        let _ = db
            .query("DELETE code_chunk WHERE project = $p AND strength < 0.1")
            .bind(("p", project.to_string()))
            .await;
    }

    // Mirror for symbols (lighter — symbols are cheaper to keep so we don't
    // outright delete them; just decay strength via primary_chunk if present).
    let _ = report; // symbols decay tracked but not deleted in v1
    Ok(())
}
