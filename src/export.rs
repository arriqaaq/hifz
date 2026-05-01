//! Export — bundle every relevant table into one JSON for backup or migration.

use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::models::ExportReq;

/// Build the export bundle. Filters apply to the observation slice; sessions,
/// memories etc. are filtered only by project (when supplied).
pub async fn run(db: &Surreal<Db>, params: ExportReq) -> Result<serde_json::Value> {
    // Observations — full filter set.
    let mut obs_conditions: Vec<String> = Vec::new();
    if let Some(ref sid) = params.session_id {
        let sid_clean = sid
            .strip_prefix("session:")
            .unwrap_or(sid)
            .replace('\'', "");
        obs_conditions.push(format!(
            "session_id = type::record('session:{}')",
            sid_clean
        ));
    }
    if params.project.is_some() {
        obs_conditions.push("project = $project".to_string());
    }
    if let Some(ref types) = params.obs_type {
        let parts: Vec<String> = types
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| format!("'{}'", s.replace('\'', "")))
            .collect();
        if !parts.is_empty() {
            obs_conditions.push(format!("obs_type IN [{}]", parts.join(", ")));
        }
    }
    if params.since.is_some() {
        obs_conditions.push("timestamp >= $since".to_string());
    }
    if params.until.is_some() {
        obs_conditions.push("timestamp <= $until".to_string());
    }
    if let Some(min_imp) = params.min_importance {
        obs_conditions.push(format!("importance >= {}", min_imp));
    }
    let obs_where = if obs_conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", obs_conditions.join(" AND "))
    };
    let obs_sql = format!("SELECT * FROM observation{}", obs_where);

    let mut obs_q = db.query(&obs_sql);
    if let Some(ref project) = params.project {
        obs_q = obs_q.bind(("project", project.clone()));
    }
    if let Some(ref since) = params.since {
        obs_q = obs_q.bind(("since", since.clone()));
    }
    if let Some(ref until) = params.until {
        obs_q = obs_q.bind(("until", until.clone()));
    }
    let observations: Vec<serde_json::Value> = obs_q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    // Sessions — project filter only.
    let sessions_sql = if params.project.is_some() {
        "SELECT * FROM session WHERE project = $project"
    } else {
        "SELECT * FROM session"
    };
    let mut s_q = db.query(sessions_sql);
    if let Some(ref project) = params.project {
        s_q = s_q.bind(("project", project.clone()));
    }
    let sessions: Vec<serde_json::Value> = s_q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    // Memories — project filter with global fallback.
    let memories_sql = if params.project.is_some() {
        "SELECT * FROM memory WHERE is_latest = true AND (project = $project OR project = 'global')"
    } else {
        "SELECT * FROM memory WHERE is_latest = true"
    };
    let mut m_q = db.query(memories_sql);
    if let Some(ref project) = params.project {
        m_q = m_q.bind(("project", project.clone()));
    }
    let memories: Vec<serde_json::Value> = m_q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let semantic: Vec<serde_json::Value> = db
        .query("SELECT * FROM semantic_memory")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let procedural: Vec<serde_json::Value> = db
        .query("SELECT * FROM procedural_memory")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let runs: Vec<serde_json::Value> = db
        .query("SELECT * FROM run ORDER BY started_at DESC")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let commits: Vec<serde_json::Value> = db
        .query("SELECT * FROM observation WHERE obs_type = 'commit_made' ORDER BY timestamp DESC")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "sessions": sessions,
        "observations": observations,
        "memories": memories,
        "semantic_memories": semantic,
        "procedural_memories": procedural,
        "runs": runs,
        "commits": commits,
    }))
}
