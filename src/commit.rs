use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, RecordIdKey, SurrealValue};

use crate::db::Db;
use crate::ground;
use crate::plan;
use crate::run;

/// Extract the key portion of a RecordId as a string.
fn record_id_key_string(id: &RecordId) -> String {
    match &id.key {
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::Uuid(u) => u.to_string(),
        _ => format!("{:?}", id.key),
    }
}

/// Data needed to record a commit (from git_detect or the POST endpoint).
#[derive(Debug, Clone)]
pub struct CommitData {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub branch: String,
    pub project: String,
    pub files_changed: Vec<String>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub is_amend: bool,
    pub timestamp: String,
}

/// Record a commit in the database, enriching from git if needed.
///
/// Returns the new commit RecordId, or None if it was a duplicate.
pub async fn record_commit(
    db: &Surreal<Db>,
    mut data: CommitData,
    session_id: Option<RecordId>,
    run_id: Option<RecordId>,
    git_path: Option<&Path>,
) -> Result<Option<RecordId>> {
    // Dedup by SHA
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query("SELECT id FROM commit WHERE sha = $sha LIMIT 1")
        .bind(("sha", data.sha.clone()))
        .await?;
    let existing: Vec<Row> = resp.take(0).unwrap_or_default();
    if existing.iter().any(|r| r.id.is_some()) {
        return Ok(None);
    }

    // Git enrichment fallback: if files_changed is empty and we have a git binary
    if data.files_changed.is_empty() {
        if let Some(git) = git_path {
            enrich_from_git(git, &mut data);
        }
    }

    let now = chrono::Utc::now().to_rfc3339();

    let mut resp = db
        .query(
            "CREATE commit SET \
             sha = $sha, message = $message, author = $author, \
             branch = $branch, project = $project, \
             files_changed = $files_changed, \
             insertions = $insertions, deletions = $deletions, \
             is_amend = $is_amend, \
             session_id = $session_id, run_id = $run_id, \
             timestamp = $timestamp, created_at = $now \
             RETURN id",
        )
        .bind(("sha", data.sha.clone()))
        .bind(("message", data.message.clone()))
        .bind(("author", data.author.clone()))
        .bind(("branch", data.branch.clone()))
        .bind(("project", data.project.clone()))
        .bind(("files_changed", data.files_changed.clone()))
        .bind(("insertions", data.insertions))
        .bind(("deletions", data.deletions))
        .bind(("is_amend", data.is_amend))
        .bind(("session_id", session_id.clone()))
        .bind(("run_id", run_id.clone()))
        .bind(("timestamp", data.timestamp.clone()))
        .bind(("now", now))
        .await?;
    resp = resp.check()?;
    let created: Vec<Row> = resp.take(0).unwrap_or_default();
    let commit_id = created.into_iter().next().and_then(|r| r.id);

    let Some(ref cid) = commit_id else {
        return Ok(None);
    };

    // Mark the open run as committed (but don't close - work may continue)
    if let Some(ref rid) = run_id {
        let short_sha = &data.sha[..data.sha.len().min(7)];
        let _ = run::mark_committed(db, rid, cid, short_sha, &data.message).await;
    }

    // Strengthen related memories
    if session_id.is_some() {
        let _ = ground::on_commit(db, &data.project, &data.files_changed).await;
    }

    // Link commit to active plan and check for auto-completion
    if let Ok(Some(active_plan)) = plan::get_active(db, &data.project).await {
        if let Some(ref plan_id) = active_plan.id {
            let plan_files: HashSet<_> = active_plan
                .files
                .as_ref()
                .map(|f| f.iter().collect())
                .unwrap_or_default();
            let commit_files: HashSet<_> = data.files_changed.iter().collect();
            let overlap = plan_files.intersection(&commit_files).count();

            if overlap > 0 {
                // Link commit to plan
                let plan_key = record_id_key_string(plan_id);
                let _ = plan::link_commit(db, &plan_key, &plan_key).await;

                // Check if plan is complete (80%+ of plan files committed)
                if !plan_files.is_empty() {
                    let coverage = overlap as f64 / plan_files.len() as f64;
                    if coverage >= 0.8 {
                        let commit_key = record_id_key_string(cid);
                        let _ = plan::complete(db, &plan_key, Some(&commit_key)).await;
                    }
                }
            }
        }
    }

    // Populate summary table for consolidation pipeline
    if let Some(ref rid) = run_id {
        if let Some(ref sid) = session_id {
            let _ = create_summary_from_run(db, rid, sid, &data.project, &data.message).await;
        }
    }

    Ok(commit_id)
}

/// Build a summary row from a committed run, feeding tier_semantic.
pub async fn create_summary_from_run(
    db: &Surreal<Db>,
    run_id: &RecordId,
    session_id: &RecordId,
    project: &str,
    commit_message: &str,
) -> Result<()> {
    #[derive(Debug, SurrealValue)]
    struct ObsRow {
        title: Option<String>,
        narrative: Option<String>,
        files: Option<Vec<String>>,
        keywords: Option<Vec<String>>,
        importance: Option<i64>,
    }

    let mut resp = db
        .query(
            "SELECT title, narrative, files, keywords, importance FROM observation \
             WHERE id IN (SELECT VALUE observation_ids FROM type::record($rid))[0] \
             ORDER BY importance DESC LIMIT 10",
        )
        .bind(("rid", run_id.clone()))
        .await?;
    let obs: Vec<ObsRow> = resp.take(0).unwrap_or_default();

    if obs.is_empty() {
        return Ok(());
    }

    let narratives: Vec<String> = obs
        .iter()
        .take(5)
        .filter_map(|o| o.narrative.clone())
        .collect();
    let all_files: Vec<String> = obs
        .iter()
        .flat_map(|o| o.files.clone().unwrap_or_default())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let all_keywords: Vec<String> = obs
        .iter()
        .flat_map(|o| o.keywords.clone().unwrap_or_default())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let key_decisions: Vec<String> = obs.iter().filter_map(|o| o.title.clone()).take(5).collect();

    let now = chrono::Utc::now().to_rfc3339();

    db.query(
        "CREATE summary SET \
         session_id = $sid, project = $project, created_at = $now, \
         title = $title, narrative = $narrative, \
         key_decisions = $key_decisions, files_modified = $files, \
         keywords = $keywords, observation_count = $obs_count",
    )
    .bind(("sid", session_id.clone()))
    .bind(("project", project.to_string()))
    .bind(("now", now))
    .bind(("title", commit_message.to_string()))
    .bind(("narrative", narratives.join(" ")))
    .bind(("key_decisions", key_decisions))
    .bind(("files", all_files))
    .bind(("keywords", all_keywords))
    .bind(("obs_count", obs.len() as i64))
    .await?
    .check()?;

    Ok(())
}

fn enrich_from_git(git: &Path, data: &mut CommitData) {
    // Single command: --numstat gives machine-parseable files + stats
    if let Ok(output) = Command::new(git)
        .args(["diff-tree", "--numstat", "--no-commit-id", "-r", &data.sha])
        .current_dir(&data.project)
        .output()
    {
        if output.status.success() {
            let (files, ins, del) = parse_numstat(&output.stdout);
            data.files_changed = files;
            data.insertions = Some(ins);
            data.deletions = Some(del);
        }
    }

    if data.author.is_empty() {
        if let Ok(output) = Command::new(git)
            .args(["log", "-1", "--format=%an <%ae>", &data.sha])
            .current_dir(&data.project)
            .output()
        {
            if output.status.success() {
                data.author = String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
        }
    }
}

fn parse_numstat(stdout: &[u8]) -> (Vec<String>, i64, i64) {
    let mut files = Vec::new();
    let mut total_ins: i64 = 0;
    let mut total_del: i64 = 0;
    for line in String::from_utf8_lossy(stdout).lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3 {
            total_ins += parts[0].parse::<i64>().unwrap_or(0); // "-" for binary files
            total_del += parts[1].parse::<i64>().unwrap_or(0);
            files.push(parts[2].to_string());
        }
    }
    (files, total_ins, total_del)
}
