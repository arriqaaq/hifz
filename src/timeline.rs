//! Timeline view — observations, optionally filtered to one session.

use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::models::TimelineReq;

/// List observations (most recent first when no session filter, oldest first
/// within a session). Mirrors the legacy `/api/v1/agent/timeline` shape.
pub async fn list(db: &Surreal<Db>, params: TimelineReq) -> Result<serde_json::Value> {
    let session_id = params.session_id.as_deref().unwrap_or("");
    let limit = params.limit.unwrap_or(50);

    let sql = if session_id.is_empty() {
        format!("SELECT * FROM observation ORDER BY timestamp DESC LIMIT {limit}")
    } else {
        let sid_clean = session_id.strip_prefix("session:").unwrap_or(session_id);
        format!(
            "SELECT * FROM observation WHERE session_id = type::record('session:{}') \
             ORDER BY timestamp ASC LIMIT {limit}",
            sid_clean.replace('\'', "")
        )
    };

    let mut resp = db.query(&sql).await?;
    let obs: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Ok(serde_json::json!({"observations": obs}))
}
