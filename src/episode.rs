//! Episodes — task-scoped trajectories within a session.
//!
//! An episode spans `UserPromptSubmit → ... → Stop / TaskCompleted`. This
//! module provides helpers to start, append, and close episodes so the
//! observation pipeline can attribute each observation to the task that
//! motivated it, and so the `hifz_episodes` tool can retrieve past tasks.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

/// Start a new episode. Returns the new episode id.
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
            "CREATE episode SET \
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

/// Append an observation id to an open episode.
pub async fn append(
    db: &Surreal<Db>,
    episode_id: &RecordId,
    observation_id: &RecordId,
) -> Result<()> {
    db.query(
        "UPDATE type::thing($eid) SET observation_ids = array::concat(observation_ids, [$oid])",
    )
    .bind(("eid", episode_id.clone()))
    .bind(("oid", observation_id.clone()))
    .await?
    .check()?;
    Ok(())
}

/// Close an episode, setting `ended_at` and deriving a lesson from the
/// highest-importance observation titles if one is not provided.
pub async fn close(
    db: &Surreal<Db>,
    episode_id: &RecordId,
    outcome: &str,
    lesson_override: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    let lesson = if let Some(l) = lesson_override {
        l.to_string()
    } else {
        derive_lesson(db, episode_id).await.unwrap_or_default()
    };

    let lesson_opt: Option<String> = if lesson.is_empty() {
        None
    } else {
        Some(lesson)
    };

    db.query("UPDATE type::thing($eid) SET ended_at = $now, outcome = $outcome, lesson = $lesson")
        .bind(("eid", episode_id.clone()))
        .bind(("now", now))
        .bind(("outcome", outcome.to_string()))
        .bind(("lesson", lesson_opt))
        .await?
        .check()?;
    Ok(())
}

/// Deterministic lesson: concatenate titles of the episode's highest-importance
/// observations. LLM evolution (Phase 5) can upgrade this later.
async fn derive_lesson(db: &Surreal<Db>, episode_id: &RecordId) -> Result<String> {
    #[derive(Debug, SurrealValue)]
    struct ObsRow {
        title: Option<String>,
        importance: Option<i64>,
    }

    let mut resp = db
        .query(
            "SELECT title, importance FROM observation \
             WHERE id IN (SELECT VALUE observation_ids FROM type::thing($eid))[0] \
             ORDER BY importance DESC LIMIT 5",
        )
        .bind(("eid", episode_id.clone()))
        .await?;
    let rows: Vec<ObsRow> = resp.take(0).unwrap_or_default();
    let titles: Vec<String> = rows.into_iter().filter_map(|r| r.title).collect();
    Ok(titles.join(" · "))
}

/// Hybrid search across `episode.prompt` and `episode.lesson`, project-scoped.
pub async fn search(
    db: &Surreal<Db>,
    project: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    let project_filter = if project.is_some() {
        " AND project = $project"
    } else {
        ""
    };

    let sql = format!(
        "search::rrf([\
             (SELECT id, search::score(1) AS ft_score \
              FROM episode WHERE prompt @1,OR@ $q{project_filter} \
              ORDER BY ft_score DESC LIMIT {limit}),\
             (SELECT id, search::score(2) AS ft_score \
              FROM episode WHERE lesson @2,OR@ $q{project_filter} \
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
        .query("SELECT * FROM episode WHERE id IN $ids")
        .bind(("ids", ids))
        .await?;
    let rows: Vec<serde_json::Value> = fetch.take(0).unwrap_or_default();
    Ok(rows)
}
