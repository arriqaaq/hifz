//! Memory grounding — the mechanism that makes memories mortal.
//!
//! Committed work strengthens memories. Uncommitted work fades on session end.
//! The existing forget.rs GC handles actual deletion of expired memories.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

/// Strength multiplier for memories a commit confirms (add/modify).
const GROUND_BOOST: f64 = 1.15;

/// Strength multiplier for memories a commit *contradicts* (files reverted
/// or deleted). `< 1.0` weakens. Proven by the `ground` benchmark gate
/// (false-advice 0.333→0.000, recall unchanged), so it is unconditional —
/// no flag.
const GROUND_WEAKEN: f64 = 0.5;

/// Polarity-aware view of a `commit_made` observation, parsed from the
/// adapter `metadata`. Backward-compatible: a payload without `metadata`
/// (older adapter / queued observation) yields all-add / no-revert, which
/// reproduces the prior boost-only behavior exactly.
#[derive(Debug, Default, Clone)]
pub struct CommitSignal {
    pub files_added_modified: Vec<String>,
    pub files_removed: Vec<String>,
    pub is_revert: bool,
}

impl CommitSignal {
    /// Build from observation `metadata` (the adapter's `{file_status,
    /// is_revert, files}`). `fallback_files` is `compressed.files` and is
    /// used (as all add/modify) when `metadata` is absent or lacks
    /// `file_status` — preserving today's behavior for old payloads.
    pub fn from_metadata(meta: Option<&serde_json::Value>, fallback_files: &[String]) -> Self {
        let Some(meta) = meta else {
            return Self {
                files_added_modified: fallback_files.to_vec(),
                files_removed: Vec::new(),
                is_revert: false,
            };
        };
        let is_revert = meta
            .get("is_revert")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let status = meta.get("file_status").and_then(|v| v.as_array());
        let Some(status) = status else {
            // metadata present but no per-file status: treat all as
            // add/modify (old adapter shape) — unchanged behavior.
            return Self {
                files_added_modified: meta
                    .get("files")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_else(|| fallback_files.to_vec()),
                files_removed: Vec::new(),
                is_revert,
            };
        };
        let mut added_modified = Vec::new();
        let mut removed = Vec::new();
        for entry in status {
            let path = entry.get("path").and_then(|v| v.as_str());
            let st = entry.get("status").and_then(|v| v.as_str()).unwrap_or("M");
            if let Some(p) = path {
                if st == "D" {
                    removed.push(p.to_string());
                } else {
                    added_modified.push(p.to_string());
                }
            }
        }
        Self {
            files_added_modified: added_modified,
            files_removed: removed,
            is_revert,
        }
    }

    /// Files whose associated memories a revert/deletion contradicts: on a
    /// revert, everything the revert touched; otherwise just deletions.
    fn contradicted_files(&self) -> Vec<String> {
        if self.is_revert {
            let mut v = self.files_added_modified.clone();
            v.extend(self.files_removed.iter().cloned());
            v
        } else {
            self.files_removed.clone()
        }
    }
}

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
             strength = math::min([strength * $boost, 1.0])",
        )
        .bind(("id", id))
        .bind(("boost", GROUND_BOOST))
        .await?;
        strengthened += 1;
    }

    if strengthened > 0 {
        tracing::info!("ground::on_commit_observation: strengthened {strengthened} memories");
    }

    // Code dimension (M5+, G8): bump strength + last_committed_at for chunks
    // whose `path` matches the committed file set. This keeps cold-decay GC
    // from sweeping chunks that just got real-world reinforcement.
    #[cfg(feature = "code")]
    {
        let now = chrono::Utc::now().to_rfc3339();
        let _ = db
            .query(
                "UPDATE code_chunk SET \
                   strength = math::min([strength * 1.10, 1.0]), \
                   last_committed_at = $now \
                 WHERE project = $project AND path IN $files",
            )
            .bind(("project", project.to_string()))
            .bind(("files", files_changed.to_vec()))
            .bind(("now", now.clone()))
            .await;
        let _ = db
            .query(
                "UPDATE code_file SET last_committed_at = $now \
                 WHERE project = $project AND path IN $files",
            )
            .bind(("project", project.to_string()))
            .bind(("files", files_changed.to_vec()))
            .bind(("now", now))
            .await;
    }

    Ok(strengthened)
}

/// Outcome of processing one `commit_made` signal.
#[derive(Debug, Default)]
pub struct GroundReport {
    pub strengthened: usize,
    /// Memories a revert/deletion contradicted (and weakened).
    pub weakened: usize,
}

/// Polarity-aware grounding for a `commit_made` observation.
///
/// - Non-revert add/modify  → confirm  → strengthen.
/// - Revert / file-deletion → contradict → *weaken* the memories that
///   described the now-undone approach. THIS is the polarity-bug fix: a
///   revert previously strengthened the very memory it invalidated.
///
/// Unconditional: proven by the `ground` benchmark gate (false-advice
/// 0.333→0.000, recall unchanged). `strength` stays a native `[0,1]`
/// factor.
pub async fn on_commit_signal(
    db: &Surreal<Db>,
    project: &str,
    sig: &CommitSignal,
) -> Result<GroundReport> {
    let mut report = GroundReport::default();

    // Confirm path: only strengthen on a non-revert commit's add/modify set.
    if !sig.is_revert && !sig.files_added_modified.is_empty() {
        report.strengthened = on_commit_observation(db, project, &sig.files_added_modified).await?;
    }

    // Contradict path: revert or deletion.
    let contradicted = sig.contradicted_files();
    if contradicted.is_empty() {
        return Ok(report);
    }

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query(
            "SELECT id FROM memory \
             WHERE is_latest = true \
               AND pinned = false \
               AND project = $project \
               AND array::intersect(files, $files) != [] \
             LIMIT 50",
        )
        .bind(("project", project.to_string()))
        .bind(("files", contradicted.clone()))
        .await?;
    let rows: Vec<RecordId> = resp
        .take::<Vec<Row>>(0)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| r.id)
        .collect();

    if rows.is_empty() {
        return Ok(report);
    }
    report.weakened = rows.len();

    // `strength` ≥ 0 and `GROUND_WEAKEN` ∈ [0,1), so the product stays in
    // `[0, strength]` — no clamp needed. (SurrealDB's `math::min`/`max`
    // take a single array arg, not two scalars.)
    for id in rows {
        db.query("UPDATE type::record($id) SET strength = strength * $w")
            .bind(("id", id))
            .bind(("w", GROUND_WEAKEN))
            .await?;
    }
    tracing::info!(
        "ground::on_commit_signal: weakened {} memories (revert/delete, factor {GROUND_WEAKEN})",
        report.weakened
    );
    Ok(report)
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
