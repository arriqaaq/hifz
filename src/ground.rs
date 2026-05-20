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

/// Files referenced by more than this many memories are "structural"
/// (e.g. `Cargo.toml`, `mod.rs`, `lib.rs`) — they do not discriminate which
/// memory a commit is *about*, so they must not confer watermark protection
/// (over-protection) nor blanket-weaken on revert. Mirrors the shipped
/// hot-file cap in `observe.rs::link_observation_files_to_memories`
/// ("so a hot file (e.g. `Cargo.toml`) doesn't link to every memory"). It is
/// a count cap, not a tuned score — deterministic, no empirical gate needed.
const HOT_FILE_MAX: i64 = 20;

/// Drop hot/structural files (referenced by > `HOT_FILE_MAX` memories) from a
/// committed file set. Per-file: specific files survive; only structural ones
/// are removed. A commit touching *only* structural files yields an empty set
/// (correctly grounds nothing — e.g. a pure version-bump commit).
async fn discriminating_files(db: &Surreal<Db>, project: &str, files: &[String]) -> Vec<String> {
    #[derive(Debug, SurrealValue)]
    struct CountRow {
        c: Option<i64>,
    }
    let mut keep = Vec::new();
    for f in files {
        let trimmed = f.trim();
        if trimmed.is_empty() {
            continue;
        }
        let count = match db
            .query(
                "SELECT count() AS c FROM memory \
                 WHERE is_latest = true AND project = $project AND $file IN files \
                 GROUP ALL",
            )
            .bind(("project", project.to_string()))
            .bind(("file", trimmed.to_string()))
            .await
        {
            Ok(mut r) => r
                .take::<Vec<CountRow>>(0)
                .ok()
                .and_then(|v| v.into_iter().next())
                .and_then(|r| r.c)
                .unwrap_or(0),
            Err(e) => {
                tracing::debug!("ground: hot-file count failed for '{trimmed}': {e}");
                0
            }
        };
        if count > HOT_FILE_MAX {
            tracing::debug!(
                "ground: skipping structural file '{trimmed}' ({count} memories reference it, > {HOT_FILE_MAX})"
            );
        } else {
            keep.push(trimmed.to_string());
        }
    }
    keep
}

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

    // Hot-file dampening: a structural file (Cargo.toml/mod.rs/…) touched by
    // an unrelated commit must not immortalize every memory that ever
    // mentioned it. Drop those before grounding (per-file; specific files
    // still confer protection).
    let files = discriminating_files(db, project, files_changed).await;
    if files.is_empty() {
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
        .bind(("files", files.clone()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();

    // Outcome-grounding watermark: stamp the commit time and clear any pending
    // fade. `last_committed_at` (not the soft strength nudge) is what makes the
    // memory persist — set on the existing *synchronous* commit path, so it is
    // crash-safe with no spawn/ledger. Idempotent by algebra: re-applying the
    // same commit just rewrites the timestamp and re-clears NONE — safe under
    // replay / Claude+git-hook dual delivery with zero dedup. Reverts never
    // reach here (on_commit_signal only calls this on the non-revert confirm
    // set), so a reverted memory simply loses protection and fades normally.
    let committed_at = chrono::Utc::now().to_rfc3339();
    let mut strengthened = 0;
    for row in rows {
        let Some(id) = row.id else { continue };
        db.query(
            "UPDATE type::record($id) SET \
             strength = math::min([strength * $boost, 1.0]), \
             last_committed_at = $committed_at, \
             forget_after = NONE",
        )
        .bind(("id", id))
        .bind(("boost", GROUND_BOOST))
        .bind(("committed_at", committed_at.clone()))
        .await?;
        strengthened += 1;
    }

    if strengthened > 0 {
        tracing::info!("ground::on_commit_observation: strengthened {strengthened} memories");
    }

    // Code dimension: bump strength + last_committed_at for chunks
    // whose `path` matches the committed file set. This keeps cold-decay GC
    // from sweeping chunks that just got real-world reinforcement.
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
            .bind(("files", files.clone()))
            .bind(("now", now.clone()))
            .await;
        let _ = db
            .query(
                "UPDATE code_file SET last_committed_at = $now \
                 WHERE project = $project AND path IN $files",
            )
            .bind(("project", project.to_string()))
            .bind(("files", files.clone()))
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

    // Contradict path: revert or deletion. Same hot-file dampening as the
    // confirm path — a structural-file revert must not blanket-weaken every
    // memory that mentioned it.
    let contradicted = sig.contradicted_files();
    if contradicted.is_empty() {
        return Ok(report);
    }
    let contradicted = discriminating_files(db, project, &contradicted).await;
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
            // Protection is now the memory's own `last_committed_at`
            // watermark. The old correlated subquery filtered
            // `observation.project` — a column that does not exist on the
            // SCHEMAFULL `observation` table — so it never matched and the
            // guard was silently dead (the long-standing K1 bug). Deleted,
            // not patched: a memory that was ever committed (anywhere) has
            // `last_committed_at` set and is not eligible to fade.
            "SELECT id FROM memory \
             WHERE is_latest = true \
               AND pinned = false \
               AND forget_after IS NONE \
               AND last_committed_at IS NONE \
               AND array::intersect(files, $files) != [] \
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
