//! Runs — task-scoped trajectories within a session.
//!
//! A run spans `UserPromptSubmit → ... → Stop / TaskCompleted`. This
//! module provides helpers to start, append, and close runs so the
//! observation pipeline can attribute each observation to the task that
//! motivated it, and so the `hifz_runs` tool can retrieve past tasks.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

/// Start a new run. Returns the new run id.
pub async fn start(
    db: &Surreal<Db>,
    session_id: &RecordId,
    project: &str,
    prompt: &str,
) -> Result<Option<RecordId>> {
    let now = chrono::Utc::now().to_rfc3339();
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let resp = db
        .query(
            "CREATE run SET \
             session_id = $sid, project = $project, prompt = $prompt, \
             outcome = 'unknown', started_at = $now, observation_ids = [] \
             RETURN id",
        )
        .bind(("sid", session_id.clone()))
        .bind(("project", project.to_string()))
        .bind(("prompt", prompt.to_string()))
        .bind(("now", now))
        .await?;
    let rows: Vec<Row> = resp.check()?.take(0).unwrap_or_default();
    Ok(rows.into_iter().next().and_then(|r| r.id))
}

/// Append an observation id to an open run.
pub async fn append(db: &Surreal<Db>, run_id: &RecordId, observation_id: &RecordId) -> Result<()> {
    db.query(
        "UPDATE type::record($rid) SET observation_ids = array::concat(observation_ids, [$oid])",
    )
    .bind(("rid", run_id.clone()))
    .bind(("oid", observation_id.clone()))
    .await?
    .check()?;
    Ok(())
}

/// Append a prompt to an existing run (multi-prompt support).
pub async fn append_prompt(db: &Surreal<Db>, run_id: &RecordId, prompt: &str) -> Result<()> {
    db.query("UPDATE type::record($rid) SET prompts = array::concat(prompts ?? [], [$prompt])")
        .bind(("rid", run_id.clone()))
        .bind(("prompt", prompt.to_string()))
        .await?
        .check()?;
    Ok(())
}

/// Set the plan_id on a run.
pub async fn set_plan(db: &Surreal<Db>, run_id: &RecordId, plan_id: &RecordId) -> Result<()> {
    db.query("UPDATE type::record($rid) SET plan_id = $pid")
        .bind(("rid", run_id.clone()))
        .bind(("pid", plan_id.clone()))
        .await?
        .check()?;
    Ok(())
}

/// Close a run, setting `ended_at` and deriving a lesson from the
/// highest-importance observation titles if one is not provided.
pub async fn close(
    db: &Surreal<Db>,
    run_id: &RecordId,
    outcome: &str,
    lesson_override: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    let lesson = if let Some(l) = lesson_override {
        l.to_string()
    } else {
        derive_lesson(db, run_id).await.unwrap_or_default()
    };

    let lesson_opt: Option<String> = if lesson.is_empty() {
        None
    } else {
        Some(lesson)
    };

    db.query("UPDATE type::record($rid) SET ended_at = $now, outcome = $outcome, lesson = $lesson")
        .bind(("rid", run_id.clone()))
        .bind(("now", now))
        .bind(("outcome", outcome.to_string()))
        .bind(("lesson", lesson_opt))
        .await?
        .check()?;
    Ok(())
}

/// Close a run with a commit, setting commit_id and using the commit
/// message as the lesson.
pub async fn close_with_commit(
    db: &Surreal<Db>,
    run_id: &RecordId,
    commit_id: &RecordId,
    short_sha: &str,
    message: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let lesson = format!("Committed {short_sha}: {message}");

    db.query(
        "UPDATE type::record($rid) SET \
         ended_at = $now, outcome = 'committed', \
         commit_id = $cid, lesson = $lesson",
    )
    .bind(("rid", run_id.clone()))
    .bind(("now", now))
    .bind(("cid", commit_id.clone()))
    .bind(("lesson", lesson))
    .await?
    .check()?;
    Ok(())
}

/// Mark a run as committed without closing it (work may continue).
/// Sets commit_id and outcome but does NOT set ended_at.
pub async fn mark_committed(
    db: &Surreal<Db>,
    run_id: &RecordId,
    commit_id: &RecordId,
    short_sha: &str,
    message: &str,
) -> Result<()> {
    let lesson = format!("Committed {short_sha}: {message}");

    db.query(
        "UPDATE type::record($rid) SET \
         outcome = 'committed', \
         commit_id = $cid, lesson = $lesson",
    )
    .bind(("rid", run_id.clone()))
    .bind(("cid", commit_id.clone()))
    .bind(("lesson", lesson))
    .await?
    .check()?;
    Ok(())
}

/// Detect whether a run being closed should be marked "uncommitted".
/// Returns "uncommitted" if the run has file-write observations but no
/// commit_id, otherwise "success".
pub async fn detect_uncommitted_outcome(db: &Surreal<Db>, run_id: &RecordId) -> String {
    // Check if run already has a commit
    #[derive(Debug, SurrealValue)]
    struct RunCheck {
        commit_id: Option<RecordId>,
    }
    let mut resp = match db
        .query("SELECT commit_id FROM type::record($rid)")
        .bind(("rid", run_id.clone()))
        .await
    {
        Ok(r) => r,
        Err(_) => return "success".to_string(),
    };
    let rows: Vec<RunCheck> = resp.take(0).unwrap_or_default();
    if rows.first().and_then(|r| r.commit_id.as_ref()).is_some() {
        return "committed".to_string();
    }

    // Check if run has file-write observations
    #[derive(Debug, SurrealValue)]
    struct CountRow {
        c: Option<i64>,
    }
    let mut resp = match db
        .query(
            "SELECT count() AS c FROM observation \
             WHERE id IN (SELECT VALUE observation_ids FROM type::record($rid))[0] \
               AND obs_type IN ['file_write', 'file_edit'] \
             GROUP ALL",
        )
        .bind(("rid", run_id.clone()))
        .await
    {
        Ok(r) => r,
        Err(_) => return "success".to_string(),
    };
    let counts: Vec<CountRow> = resp.take(0).unwrap_or_default();
    let has_writes = counts.first().and_then(|r| r.c).unwrap_or(0) > 0;

    if has_writes {
        "uncommitted".to_string()
    } else {
        "success".to_string()
    }
}

/// Deterministic lesson: concatenate titles of the run's highest-importance
/// observations. LLM evolution (Phase 5) can upgrade this later.
async fn derive_lesson(db: &Surreal<Db>, run_id: &RecordId) -> Result<String> {
    #[derive(Debug, SurrealValue)]
    struct ObsRow {
        title: Option<String>,
        importance: Option<i64>,
    }

    let mut resp = db
        .query(
            "SELECT title, importance FROM observation \
             WHERE id IN (SELECT VALUE observation_ids FROM type::record($rid))[0] \
             ORDER BY importance DESC LIMIT 5",
        )
        .bind(("rid", run_id.clone()))
        .await?;
    let rows: Vec<ObsRow> = resp.take(0).unwrap_or_default();
    let titles: Vec<String> = rows.into_iter().filter_map(|r| r.title).collect();
    Ok(titles.join(" · "))
}

/// Search runs. Plain listing for wildcard/empty queries, BM25 for specific terms.
pub async fn search(
    db: &Surreal<Db>,
    project: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    let is_wildcard = query.trim().is_empty() || query.trim() == "*";

    if is_wildcard {
        let project_filter = if project.is_some() {
            " WHERE project = $project"
        } else {
            ""
        };
        let sql =
            format!("SELECT * FROM run{project_filter} ORDER BY started_at DESC LIMIT {limit}");
        let mut q = db.query(&sql);
        if let Some(p) = project {
            q = q.bind(("project", p.to_string()));
        }
        let mut resp = q.await?;
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        return Ok(rows);
    }

    let project_filter = if project.is_some() {
        " AND project = $project"
    } else {
        ""
    };

    let sql = format!(
        "search::rrf([\
             (SELECT id, search::score(1) AS ft_score \
              FROM run WHERE prompt @1,OR@ $q{project_filter} \
              ORDER BY ft_score DESC LIMIT {limit}),\
             (SELECT id, search::score(2) AS ft_score \
              FROM run WHERE lesson @2,OR@ $q{project_filter} \
              ORDER BY ft_score DESC LIMIT {limit})\
         ], {limit}, 60)"
    );

    let mut q = db.query(&sql).bind(("q", query.to_string()));
    if let Some(p) = project {
        q = q.bind(("project", p.to_string()));
    }
    let mut resp = q.await?;

    #[derive(Debug, SurrealValue)]
    struct RrfRow {
        id: Option<RecordId>,
    }
    let fused: Vec<RrfRow> = resp.take(0).unwrap_or_default();
    let ids: Vec<RecordId> = fused.into_iter().filter_map(|r| r.id).collect();
    if ids.is_empty() {
        return Ok(vec![]);
    }

    let mut fetch = db
        .query("SELECT * FROM run WHERE id IN $ids")
        .bind(("ids", ids))
        .await?;
    let rows: Vec<serde_json::Value> = fetch.take(0).unwrap_or_default();
    Ok(rows)
}

/// Run data for context injection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, SurrealValue)]
pub struct RunWithLesson {
    pub id: Option<RecordId>,
    pub prompt: Option<String>,
    pub prompts: Option<Vec<String>>,
    pub lesson: Option<String>,
    pub outcome: Option<String>,
    pub commit_id: Option<RecordId>,
    pub ended_at: Option<String>,
}

/// Get recent closed runs with lessons for context injection.
pub async fn recent_with_lessons(
    db: &Surreal<Db>,
    project: &str,
    _query: Option<&str>,
    limit: usize,
) -> Result<Vec<RunWithLesson>> {
    // For now, just get recent runs by ended_at
    // TODO: Add BM25 search when query is provided
    let mut resp = db
        .query(
            "SELECT id, prompt, prompts, lesson, outcome, commit_id, ended_at
             FROM run
             WHERE ended_at IS NOT NONE 
               AND lesson IS NOT NONE 
               AND lesson != ''
               AND project = $project
             ORDER BY ended_at DESC
             LIMIT $limit",
        )
        .bind(("project", project.to_string()))
        .bind(("limit", limit))
        .await?;
    let rows: Vec<RunWithLesson> = resp.take(0).unwrap_or_default();
    Ok(rows)
}
