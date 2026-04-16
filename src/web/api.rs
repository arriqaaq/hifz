use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;

use crate::models::HookPayload;
use crate::web::AppState;

// --- Health ---

pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let uptime = state.started_at.elapsed().as_secs();

    let sessions: i64 = state
        .db
        .query("SELECT count() AS c FROM session GROUP ALL")
        .await
        .ok()
        .and_then(|mut r| r.take::<Vec<serde_json::Value>>(0).ok())
        .and_then(|v| v.first().and_then(|r| r.get("c").and_then(|c| c.as_i64())))
        .unwrap_or(0);

    let observations: i64 = state
        .db
        .query("SELECT count() AS c FROM observation GROUP ALL")
        .await
        .ok()
        .and_then(|mut r| r.take::<Vec<serde_json::Value>>(0).ok())
        .and_then(|v| v.first().and_then(|r| r.get("c").and_then(|c| c.as_i64())))
        .unwrap_or(0);

    let memories: i64 = state
        .db
        .query("SELECT count() AS c FROM hifz GROUP ALL")
        .await
        .ok()
        .and_then(|mut r| r.take::<Vec<serde_json::Value>>(0).ok())
        .and_then(|v| v.first().and_then(|r| r.get("c").and_then(|c| c.as_i64())))
        .unwrap_or(0);

    Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "sessions": sessions,
        "observations": observations,
        "memories": memories,
        "uptime_seconds": uptime,
        "embedding_provider": "fastembed",
        "embedding_dimensions": state.embedder.dimension(),
        "ollama": state.ollama.is_some(),
    }))
}

pub async fn livez() -> &'static str {
    "ok"
}

// --- Session ---

#[derive(Deserialize)]
pub struct SessionStartReq {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub project: String,
    pub cwd: String,
}

pub async fn session_start(
    State(state): State<AppState>,
    Json(body): Json<SessionStartReq>,
) -> Json<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    let sid = format!("session:{}", body.session_id);
    let _ = state
        .db
        .query(
            "CREATE type::record($sid) SET \
             project = $project, cwd = $cwd, started_at = $now, \
             status = 'active', observation_count = 0",
        )
        .bind(("sid", sid.clone()))
        .bind(("project", body.project.clone()))
        .bind(("cwd", body.cwd.clone()))
        .bind(("now", now.clone()))
        .await;

    let context = crate::context::generate_context(&state.db, &body.project, state.token_budget)
        .await
        .unwrap_or_default();

    Json(serde_json::json!({
        "sessionId": body.session_id,
        "context": context,
    }))
}

#[derive(Deserialize)]
pub struct SessionEndReq {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

pub async fn session_end(
    State(state): State<AppState>,
    Json(body): Json<SessionEndReq>,
) -> Json<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    let sid = format!("session:{}", body.session_id);
    let _ = state
        .db
        .query("UPDATE type::record($sid) SET ended_at = $now, status = 'completed'")
        .bind(("sid", sid.clone()))
        .bind(("now", now.clone()))
        .await;

    Json(serde_json::json!({"status": "ok"}))
}

pub async fn sessions_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let mut resp = state
        .db
        .query(format!(
            "SELECT * FROM session ORDER BY started_at DESC LIMIT {limit}"
        ))
        .await
        .unwrap();
    let sessions: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Json(serde_json::json!({"sessions": sessions}))
}

// --- Observe ---

pub async fn observe(
    State(state): State<AppState>,
    Json(payload): Json<HookPayload>,
) -> Json<serde_json::Value> {
    match crate::observe::observe(
        &state.db,
        &state.dedup,
        &state.embedder,
        state.ollama.as_deref(),
        state.auto_compress,
        payload,
    )
    .await
    {
        Ok(Some(title)) => Json(serde_json::json!({"status": "ok", "title": title})),
        Ok(None) => Json(serde_json::json!({"status": "duplicate"})),
        Err(e) => Json(serde_json::json!({"status": "error", "error": e.to_string()})),
    }
}

// --- Search ---

#[derive(Deserialize)]
pub struct SearchReq {
    pub query: String,
    pub limit: Option<usize>,
    pub mode: Option<String>,
}

pub async fn smart_search(
    State(state): State<AppState>,
    Json(body): Json<SearchReq>,
) -> Json<serde_json::Value> {
    let limit = body.limit.unwrap_or(10);
    let mode = body.mode.as_deref().unwrap_or("hybrid");

    let results = match mode {
        "text" => crate::search::search_text(&state.db, &body.query, limit).await,
        "semantic" => {
            crate::search::search_semantic(&state.db, &state.embedder, &body.query, limit).await
        }
        _ => crate::search::search_hybrid(&state.db, &state.embedder, &body.query, limit).await,
    };

    match results {
        Ok(r) => Json(serde_json::json!({"results": r, "count": r.len()})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Remember ---

#[derive(Deserialize)]
pub struct RememberReq {
    pub title: String,
    pub content: String,
    #[serde(rename = "type")]
    pub mem_type: Option<String>,
    pub concepts: Option<Vec<String>>,
    pub files: Option<Vec<String>>,
}

pub async fn remember(
    State(state): State<AppState>,
    Json(body): Json<RememberReq>,
) -> Json<serde_json::Value> {
    let mem_type = body.mem_type.as_deref().unwrap_or("fact");
    let concepts = body.concepts.unwrap_or_default();
    let files = body.files.unwrap_or_default();

    match crate::remember::save(
        &state.db,
        mem_type,
        &body.title,
        &body.content,
        &concepts,
        &files,
    )
    .await
    {
        Ok(title) => Json(serde_json::json!({"status": "ok", "title": title})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Forget ---

#[derive(Deserialize)]
pub struct ForgetReq {
    pub id: String,
}

pub async fn forget(
    State(state): State<AppState>,
    Json(body): Json<ForgetReq>,
) -> Json<serde_json::Value> {
    match crate::remember::forget(&state.db, &body.id).await {
        Ok(()) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Context ---

#[derive(Deserialize)]
pub struct ContextReq {
    pub project: String,
    pub token_budget: Option<usize>,
}

pub async fn context(
    State(state): State<AppState>,
    Json(body): Json<ContextReq>,
) -> Json<serde_json::Value> {
    let budget = body.token_budget.unwrap_or(state.token_budget);
    match crate::context::generate_context(&state.db, &body.project, budget).await {
        Ok(ctx) => Json(serde_json::json!({"context": ctx})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Digest (project intelligence) ---

pub async fn digest(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let project = params.get("project").map(|s| s.as_str()).unwrap_or("");
    match crate::digest::generate_digest(&state.db, project).await {
        Ok(p) => Json(serde_json::to_value(p).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Forget GC (garbage collection) ---

pub async fn forget_gc(State(state): State<AppState>) -> Json<serde_json::Value> {
    match crate::forget::run_forget(&state.db, false).await {
        Ok(r) => Json(serde_json::to_value(r).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Consolidation ---

pub async fn consolidate(State(state): State<AppState>) -> Json<serde_json::Value> {
    match crate::consolidate::run_consolidation(&state.db, state.ollama.as_deref()).await {
        Ok(r) => Json(serde_json::to_value(r).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Export ---

pub async fn export(State(state): State<AppState>) -> Json<serde_json::Value> {
    let sessions: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM session")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let observations: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM observation")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let memories: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM hifz")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let semantic: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM semantic_hifz")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let procedural: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM procedural_hifz")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "sessions": sessions,
        "observations": observations,
        "memories": memories,
        "semantic_memories": semantic,
        "procedural_memories": procedural,
    }))
}

// --- Timeline ---

pub async fn timeline(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let session_id = params.get("session_id").map(|s| s.as_str()).unwrap_or("");
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    let sql = if session_id.is_empty() {
        format!("SELECT * FROM observation ORDER BY timestamp DESC LIMIT {limit}")
    } else {
        format!(
            "SELECT * FROM observation WHERE session_id = type::record('session:{session_id}') \
             ORDER BY timestamp ASC LIMIT {limit}"
        )
    };

    let mut resp = state.db.query(&sql).await.unwrap();
    let obs: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Json(serde_json::json!({"observations": obs}))
}
