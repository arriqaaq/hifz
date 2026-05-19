//! Git commits view — observations with `obs_type = 'commit_made'`.
//!
//! Commits are produced by the adapters (Pi extension's `git.ts`, Claude Code's
//! `post-tool-use.mjs`) when they detect a successful `git commit`. Their
//! metadata field carries `{sha, branch, message, files}`.

use std::path::PathBuf;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::db::Db;
use crate::models::CommitsReq;

/// List commit_made observations with optional filters: `sha` short-circuits
/// to a single-row lookup; otherwise filter by project/branch.
pub async fn list(db: &Surreal<Db>, params: CommitsReq) -> Result<serde_json::Value> {
    if let Some(sha) = params.sha.as_deref() {
        let mut resp = db
            .query(
                "SELECT * FROM observation \
                 WHERE obs_type = 'commit_made' AND metadata.sha = $sha LIMIT 1",
            )
            .bind(("sha", sha.to_string()))
            .await?;
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        return Ok(serde_json::json!({"commits": rows}));
    }

    let project = params.project.as_deref().unwrap_or("");
    let branch = params.branch.as_deref();
    let limit = params.limit.unwrap_or(10);

    let mut conditions = vec!["obs_type = 'commit_made'".to_string()];
    if !project.is_empty() {
        // `observation` has no `project` field — it lives on the linked
        // `session`. Traverse the record link (same pattern as observe.rs).
        conditions.push("session_id.project = $project".to_string());
    }
    if branch.is_some() {
        conditions.push("metadata.branch = $branch".to_string());
    }

    let where_clause = format!(" WHERE {}", conditions.join(" AND "));
    let sql =
        format!("SELECT * FROM observation{where_clause} ORDER BY timestamp DESC LIMIT {limit}");

    let mut q = db.query(&sql);
    if !project.is_empty() {
        q = q.bind(("project", project.to_string()));
    }
    if let Some(b) = branch {
        q = q.bind(("branch", b.to_string()));
    }
    let mut resp = q.await?;
    let commits: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Ok(serde_json::json!({"commits": commits}))
}

/// Show the unified diff for a commit by SHA. Looks up the project from the
/// stored observation, then runs `git show --stat --patch` in that directory.
/// Returns `{"sha", "diff"}` on success or `{"error"}` on any failure.
pub async fn diff(
    db: &Surreal<Db>,
    git_path: Option<&PathBuf>,
    sha: &str,
) -> Result<serde_json::Value> {
    // `observation` is SCHEMAFULL with no `project` field — project lives on
    // the linked `session` (set by `ensure_session` from `payload.project`).
    // Traverse the record link, same proven pattern as `observe.rs`
    // (`session_id.project`). The old flat `SELECT project` was always NULL,
    // so every commit's diff returned "commit not found".
    let mut resp = db
        .query(
            "SELECT session_id.project AS project FROM observation \
             WHERE obs_type = 'commit_made' AND metadata.sha = $sha LIMIT 1",
        )
        .bind(("sha", sha.to_string()))
        .await?;

    #[derive(Debug, SurrealValue)]
    struct ProjectRow {
        project: Option<String>,
    }
    let rows: Vec<ProjectRow> = resp.take(0).unwrap_or_default();
    let project = match rows.into_iter().next().and_then(|r| r.project) {
        Some(p) => p,
        None => return Ok(serde_json::json!({"error": "commit not found"})),
    };

    let Some(git) = git_path else {
        return Ok(serde_json::json!({"error": "git not available"}));
    };

    let output = std::process::Command::new(git)
        .args(["show", "--stat", "--patch", "--format=", sha])
        .current_dir(&project)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let diff = String::from_utf8_lossy(&o.stdout).to_string();
            Ok(serde_json::json!({"sha": sha, "diff": diff}))
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).to_string();
            Ok(serde_json::json!({"error": err}))
        }
        Err(e) => Ok(serde_json::json!({"error": e.to_string()})),
    }
}
