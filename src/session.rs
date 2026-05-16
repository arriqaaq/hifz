//! Session lifecycle — start/end/get/list, with auto-derived session names.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::db::Db;
use crate::embed::Embedder;
use crate::models::SessionStartReq;
use crate::truncate_at_char_boundary;

/// Create a new session row and return the synthesised context for the project.
/// Mirrors the legacy handler shape: `{"sessionId", "context"}`.
pub async fn start(
    db: &Surreal<Db>,
    embedder: &Embedder,
    token_budget: usize,
    body: SessionStartReq,
) -> Result<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    let sid = format!("session:{}", body.session_id);
    let _ = db
        .query(
            "CREATE type::record($sid) SET \
             project = $project, cwd = $cwd, started_at = $now, \
             status = 'active', observation_count = 0",
        )
        .bind(("sid", sid))
        .bind(("project", body.project.clone()))
        .bind(("cwd", body.cwd))
        .bind(("now", now))
        .await;

    let context = crate::context::generate_context_with_query(
        db,
        Some(embedder),
        &body.project,
        None,
        token_budget,
    )
    .await
    .unwrap_or_default();

    Ok(serde_json::json!({
        "sessionId": body.session_id,
        "context": context,
    }))
}

/// Close a session, updating ended_at + status, and synthesise a name from the
/// first run's prompt (or fallback to highest-importance observation, or
/// project basename + date).
pub async fn end(db: &Surreal<Db>, session_id: &str) -> Result<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    let sid = if session_id.starts_with("session:") {
        session_id.to_string()
    } else {
        format!("session:{session_id}")
    };

    let name = derive_name(db, &sid).await;

    let _ = db
        .query("UPDATE type::record($sid) SET ended_at = $now, status = 'completed', name = $name")
        .bind(("sid", sid))
        .bind(("now", now))
        .bind(("name", name))
        .await;

    Ok(serde_json::json!({"status": "ok"}))
}

/// Look up one session by id (with or without `session:` prefix).
pub async fn get(db: &Surreal<Db>, id: &str) -> Result<serde_json::Value> {
    let sid = if id.starts_with("session:") {
        id.to_string()
    } else {
        format!("session:{id}")
    };

    let mut resp = db
        .query("SELECT * FROM type::record($sid)")
        .bind(("sid", sid))
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    match rows.into_iter().next() {
        Some(session) => Ok(session),
        None => Ok(serde_json::json!({"error": "session not found"})),
    }
}

/// List sessions, newest first.
pub async fn list(db: &Surreal<Db>, limit: usize) -> Result<serde_json::Value> {
    // `observation_count > 0` hides empty "ghost" sessions (a SessionStart
    // with no follow-up activity). Real sessions always have ≥1 observation
    // (the prompt that started them) and the count only increments, so this
    // only ever filters ghosts — never a session with real activity.
    let mut resp = db
        .query(format!(
            "SELECT * FROM session WHERE observation_count > 0 \
             ORDER BY started_at DESC LIMIT {limit}"
        ))
        .await?;
    let sessions: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Ok(serde_json::json!({"sessions": sessions}))
}

/// Synthesise a human-readable session name. Tries, in order:
///   1. The first run's prompt, truncated at a word boundary.
///   2. The highest-importance non-conversation observation title.
///   3. Project basename + date.
async fn derive_name(db: &Surreal<Db>, sid: &str) -> Option<String> {
    #[derive(Debug, surrealdb::types::SurrealValue)]
    struct PromptRow {
        prompt: Option<String>,
        #[allow(dead_code)]
        started_at: Option<String>,
    }
    #[derive(Debug, surrealdb::types::SurrealValue)]
    struct TitleRow {
        title: Option<String>,
        #[allow(dead_code)]
        importance: Option<i64>,
        #[allow(dead_code)]
        timestamp: Option<String>,
    }
    #[derive(Debug, surrealdb::types::SurrealValue)]
    struct ProjectRow {
        project: Option<String>,
    }

    // 1. First run prompt
    if let Ok(mut resp) = db
        .query(
            "SELECT prompt, started_at FROM run \
             WHERE session_id = type::record($sid) \
             ORDER BY started_at ASC LIMIT 1",
        )
        .bind(("sid", sid.to_string()))
        .await
    {
        let rows: Vec<PromptRow> = resp.take(0).unwrap_or_default();
        if let Some(prompt) = rows.into_iter().next().and_then(|r| r.prompt) {
            let trimmed = prompt.trim();
            if trimmed.len() > 5 {
                return Some(truncate_at_word(trimmed, 80));
            }
        }
    }

    // 2. First high-importance non-conversation observation title
    if let Ok(mut resp) = db
        .query(
            "SELECT title, importance, timestamp FROM observation \
             WHERE session_id = type::record($sid) \
               AND obs_type NOT IN ['conversation'] \
             ORDER BY importance DESC, timestamp ASC LIMIT 1",
        )
        .bind(("sid", sid.to_string()))
        .await
    {
        let rows: Vec<TitleRow> = resp.take(0).unwrap_or_default();
        if let Some(title) = rows.into_iter().next().and_then(|r| r.title) {
            let trimmed = title.trim();
            if trimmed.len() > 3 {
                return Some(truncate_at_word(trimmed, 80));
            }
        }
    }

    // 3. Last fallback: project basename + date
    if let Ok(mut resp) = db
        .query("SELECT project FROM type::record($sid)")
        .bind(("sid", sid.to_string()))
        .await
    {
        let rows: Vec<ProjectRow> = resp.take(0).unwrap_or_default();
        if let Some(project) = rows.into_iter().next().and_then(|r| r.project) {
            let basename = project.rsplit('/').next().unwrap_or(&project);
            let date = chrono::Utc::now().format("%b %d");
            return Some(format!("{basename} — {date}"));
        }
    }

    None
}

fn truncate_at_word(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let safe = truncate_at_char_boundary(s, max);
    match safe.rfind(' ') {
        Some(pos) if pos > max / 2 => format!("{}…", &safe[..pos]),
        _ => format!("{safe}…"),
    }
}
