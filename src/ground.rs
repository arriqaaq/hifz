//! Memory grounding — the mechanism that makes memories mortal.
//!
//! Committed work strengthens memories. Uncommitted work fades on session end.
//! The existing forget.rs GC handles actual deletion of expired memories.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

/// Positive signal: a commit_made observation arrived. Strengthen memories
/// that overlap with the committed files in the same project.
pub async fn on_commit_observation(
    db: &Surreal<Db>,
    project: &str,
    files_changed: &[String],
) -> Result<usize> {
    if files_changed.is_empty() {
        return Ok(0);
    }

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }

    let mut resp = db
        .query(
            "SELECT id FROM memory \
             WHERE is_latest = true \
               AND project = $project \
               AND array::intersect(files, $files) != [] \
             LIMIT 50",
        )
        .bind(("project", project.to_string()))
        .bind(("files", files_changed.to_vec()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();

    let mut strengthened = 0;
    for row in rows {
        let Some(id) = row.id else { continue };
        db.query(
            "UPDATE type::record($id) SET \
             strength = math::min(strength * 1.15, 1.0)",
        )
        .bind(("id", id))
        .await?;
        strengthened += 1;
    }

    if strengthened > 0 {
        tracing::info!("ground::on_commit_observation: strengthened {strengthened} memories");
    }

    Ok(strengthened)
}

/// Silence signal: session ended without commits. Find runs with file-write
/// observations that were never committed and set `forget_after` on linked memories.
pub async fn decay_uncommitted(db: &Surreal<Db>, session_id: &str) -> Result<usize> {
    let sid = format!("session:{session_id}");

    #[derive(Debug, SurrealValue)]
    struct RunRow {
        id: Option<RecordId>,
        observation_ids: Option<Vec<RecordId>>,
    }

    // Find runs in this session that are uncommitted (no commit_made observation)
    let mut resp = db
        .query(
            "SELECT id, observation_ids FROM run \
             WHERE session_id = type::record($sid) \
               AND (outcome = 'unknown' OR outcome = 'uncommitted')",
        )
        .bind(("sid", sid.clone()))
        .await?;
    let runs: Vec<RunRow> = resp.take(0).unwrap_or_default();

    if runs.is_empty() {
        return Ok(0);
    }

    // Collect all files from file-write observations in these runs
    let all_obs_ids: Vec<RecordId> = runs
        .iter()
        .flat_map(|r| r.observation_ids.clone().unwrap_or_default())
        .collect();

    if all_obs_ids.is_empty() {
        return Ok(0);
    }

    #[derive(Debug, SurrealValue)]
    struct FileRow {
        files: Option<Vec<String>>,
    }

    let mut resp = db
        .query(
            "SELECT files FROM observation \
             WHERE id IN $ids AND obs_type IN ['file_write', 'file_edit']",
        )
        .bind(("ids", all_obs_ids))
        .await?;
    let file_rows: Vec<FileRow> = resp.take(0).unwrap_or_default();

    let written_files: Vec<String> = file_rows
        .into_iter()
        .flat_map(|r| r.files.unwrap_or_default())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if written_files.is_empty() {
        return Ok(0);
    }

    // Set forget_after on memories whose files overlap with uncommitted writes
    // and where no commit_made observation exists for the same project+files.
    let forget_at = (chrono::Utc::now() + chrono::Duration::days(60)).to_rfc3339();

    #[derive(Debug, SurrealValue)]
    struct MemRow {
        id: Option<RecordId>,
    }

    let mut resp = db
        .query(
            "SELECT id FROM memory \
             WHERE is_latest = true \
               AND pinned = false \
               AND forget_after IS NONE \
               AND array::intersect(files, $files) != [] \
               AND (SELECT count() FROM observation \
                    WHERE obs_type = 'commit_made' \
                      AND project = $parent.project \
                      AND array::intersect(files, $parent.files) != [] \
                    GROUP ALL)[0].count = 0 \
             LIMIT 50",
        )
        .bind(("files", written_files))
        .await?;
    let mems: Vec<MemRow> = resp.take(0).unwrap_or_default();

    let mut decayed = 0;
    for mem in mems {
        let Some(id) = mem.id else { continue };
        db.query("UPDATE type::record($id) SET forget_after = $forget_at")
            .bind(("id", id))
            .bind(("forget_at", forget_at.clone()))
            .await?;
        decayed += 1;
    }

    if decayed > 0 {
        tracing::info!(
            "ground::decay_uncommitted: set forget_after on {decayed} memories (60 days)"
        );
    }

    Ok(decayed)
}
