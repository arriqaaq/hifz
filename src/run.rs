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

    // Structural edges: run --part_of--> session, run --follows--> prev_run
    #[derive(Debug, SurrealValue)]
    struct SessionRow {
        session_id: Option<RecordId>,
    }
    if let Ok(mut s_resp) = db
        .query("SELECT session_id FROM type::record($rid)")
        .bind(("rid", run_id.clone()))
        .await
    {
        let s_rows: Vec<SessionRow> = s_resp.take(0).unwrap_or_default();
        if let Some(sid) = s_rows.into_iter().next().and_then(|r| r.session_id)
            && let Err(e) = crate::link::create_run_structure_edges(db, run_id, &sid).await
        {
            tracing::warn!("run structure edges failed for {run_id:?}: {e}");
        }
    }

    Ok(())
}

/// Detect whether a run being closed should be marked "uncommitted".
/// Returns "uncommitted" if the run has file-write observations but no
/// commit_made observation, otherwise "success".
pub async fn detect_uncommitted_outcome(db: &Surreal<Db>, run_id: &RecordId) -> String {
    #[derive(Debug, SurrealValue)]
    struct CountRow {
        c: Option<i64>,
    }

    // Check if any commit_made observation belongs to this run
    let mut resp = match db
        .query(
            "SELECT count() AS c FROM observation \
             WHERE id IN (SELECT VALUE observation_ids FROM type::record($rid))[0] \
               AND obs_type = 'commit_made' \
             GROUP ALL",
        )
        .bind(("rid", run_id.clone()))
        .await
    {
        Ok(r) => r,
        Err(_) => return "success".to_string(),
    };
    let counts: Vec<CountRow> = resp.take(0).unwrap_or_default();
    if counts.first().and_then(|r| r.c).unwrap_or(0) > 0 {
        return "committed".to_string();
    }

    // Check if run has file-write observations
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
    if counts.first().and_then(|r| r.c).unwrap_or(0) > 0 {
        "uncommitted".to_string()
    } else {
        "success".to_string()
    }
}

/// Deterministic lesson: concatenate titles of the run's highest-importance
/// observations. LLM evolution can upgrade this later.
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

/// All runs for a single session, ordered by `started_at` ascending.
/// Accepts a session id with or without the `session:` prefix.
pub async fn list_by_session(db: &Surreal<Db>, session_id: &str) -> Result<Vec<serde_json::Value>> {
    let sid = if session_id.starts_with("session:") {
        session_id.to_string()
    } else {
        format!("session:{session_id}")
    };
    let mut resp = db
        .query("SELECT * FROM run WHERE session_id = type::record($sid) ORDER BY started_at ASC")
        .bind(("sid", sid))
        .await?;
    Ok(resp.take(0).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Public helpers for session/run resolution (used by remember, web/api, etc.)
// ---------------------------------------------------------------------------

/// Resolve a session ID string to a RecordId.
pub async fn resolve_session(db: &Surreal<Db>, session_id: &str) -> Option<RecordId> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let sid = if session_id.starts_with("session:") {
        session_id.to_string()
    } else {
        format!("session:{session_id}")
    };
    let mut resp = db
        .query("SELECT id FROM type::record($sid)")
        .bind(("sid", sid))
        .await
        .ok()?;
    let rows: Vec<Row> = resp.take(0).ok()?;
    rows.into_iter().next().and_then(|r| r.id)
}

/// Find the most recent open run for a session.
pub async fn find_open(db: &Surreal<Db>, session_id: &str) -> Result<Option<RecordId>> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
        #[allow(dead_code)]
        started_at: Option<String>,
    }
    let sid = if session_id.starts_with("session:") {
        session_id.to_string()
    } else {
        format!("session:{session_id}")
    };
    let mut resp = db
        .query(
            "SELECT id, started_at FROM run \
             WHERE session_id = type::record($sid) AND ended_at IS NONE \
             ORDER BY started_at DESC LIMIT 1",
        )
        .bind(("sid", sid))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    Ok(rows.into_iter().next().and_then(|r| r.id))
}

/// Get the recalled memory IDs from a run.
pub async fn get_recalled_ids(db: &Surreal<Db>, run_id: &RecordId) -> Result<Vec<RecordId>> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        recalled_ids: Option<Vec<RecordId>>,
    }
    let mut resp = db
        .query("SELECT recalled_ids FROM type::record($rid)")
        .bind(("rid", run_id.clone()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.recalled_ids)
        .unwrap_or_default())
}

/// Append recalled memory IDs to the run's context trail.
pub async fn append_recalled(
    db: &Surreal<Db>,
    run_id: &RecordId,
    memory_ids: &[RecordId],
) -> Result<()> {
    db.query(
        "UPDATE type::record($rid) SET \
         recalled_ids = array::distinct(array::concat(recalled_ids, $mids))",
    )
    .bind(("rid", run_id.clone()))
    .bind(("mids", memory_ids.to_vec()))
    .await?
    .check()?;
    Ok(())
}

/// Run data for context injection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, SurrealValue)]
pub struct RunWithLesson {
    pub id: Option<RecordId>,
    pub prompt: Option<String>,
    pub prompts: Option<Vec<String>>,
    pub lesson: Option<String>,
    pub outcome: Option<String>,
    pub ended_at: Option<String>,
}

/// Get recent closed runs with lessons for context injection.
pub async fn recent_with_lessons(
    db: &Surreal<Db>,
    project: &str,
    _query: Option<&str>,
    limit: usize,
) -> Result<Vec<RunWithLesson>> {
    let mut resp = db
        .query(
            "SELECT id, prompt, prompts, lesson, outcome, ended_at
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

// --- Run detail with observations (lifted from web/api.rs::run_detail) ---

/// Fetch one run plus all its observations, ordered by timestamp ASC.
/// Returns `{"run": {...}, "observations": [...]}` matching the legacy shape.
pub async fn detail(db: &Surreal<Db>, id: &str) -> anyhow::Result<serde_json::Value> {
    let run_rid = if id.starts_with("run:") {
        id.to_string()
    } else {
        format!("run:{id}")
    };

    let mut resp = db
        .query("SELECT * FROM type::record($rid)")
        .bind(("rid", run_rid.clone()))
        .await?;
    let runs: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let Some(run) = runs.into_iter().next() else {
        return Ok(serde_json::json!({"error": "run not found"}));
    };

    let mut obs_resp = db
        .query(
            "SELECT * FROM observation \
             WHERE id IN (SELECT VALUE observation_ids FROM type::record($rid))[0] \
             ORDER BY timestamp ASC",
        )
        .bind(("rid", run_rid))
        .await?;
    let observations: Vec<serde_json::Value> = obs_resp.take(0).unwrap_or_default();

    Ok(serde_json::json!({
        "run": run,
        "observations": observations
    }))
}
