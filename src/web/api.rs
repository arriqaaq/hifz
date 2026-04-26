use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;
use surrealdb::types::SurrealValue;

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
        .query("SELECT count() AS c FROM memory GROUP ALL")
        .await
        .ok()
        .and_then(|mut r| r.take::<Vec<serde_json::Value>>(0).ok())
        .and_then(|v| v.first().and_then(|r| r.get("c").and_then(|c| c.as_i64())))
        .unwrap_or(0);

    let commits: i64 = state
        .db
        .query("SELECT count() AS c FROM commit GROUP ALL")
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
        "commits": commits,
        "uptime_seconds": uptime,
        "embedding_provider": "fastembed",
        "embedding_dimensions": state.embedder.dimension(),
        "ollama": state.ollama.is_some(),
        "git_available": state.git_path.is_some(),
        "git_path": state.git_path.as_ref().map(|p| p.display().to_string()),
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

    let context = crate::context::generate_context_with_query(
        &state.db,
        Some(&state.embedder),
        &body.project,
        None,
        state.token_budget,
    )
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

    let name = derive_session_name(&state.db, &sid).await;

    let _ = state
        .db
        .query("UPDATE type::record($sid) SET ended_at = $now, status = 'completed', name = $name")
        .bind(("sid", sid.clone()))
        .bind(("now", now.clone()))
        .bind(("name", name))
        .await;

    Json(serde_json::json!({"status": "ok"}))
}

async fn derive_session_name(db: &surrealdb::Surreal<crate::db::Db>, sid: &str) -> Option<String> {
    #[derive(Debug, surrealdb::types::SurrealValue)]
    struct PromptRow {
        prompt: Option<String>,
    }
    #[derive(Debug, surrealdb::types::SurrealValue)]
    struct TitleRow {
        title: Option<String>,
    }
    #[derive(Debug, surrealdb::types::SurrealValue)]
    struct ProjectRow {
        project: Option<String>,
    }

    // Try first run prompt
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

    // Fallback: first high-importance non-conversation observation title
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

    // Last fallback: project basename + date
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
    match s[..max].rfind(' ') {
        Some(pos) if pos > max / 2 => format!("{}…", &s[..pos]),
        _ => format!("{}…", &s[..max]),
    }
}

pub async fn session_get(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let sid = if id.starts_with("session:") {
        id.clone()
    } else {
        format!("session:{id}")
    };

    let mut resp = match state
        .db
        .query("SELECT * FROM type::record($sid)")
        .bind(("sid", sid))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
    };
    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    match rows.into_iter().next() {
        Some(session) => Json(session),
        None => Json(serde_json::json!({"error": "session not found"})),
    }
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
        state.git_path.as_deref(),
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
    pub project: Option<String>,
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: Option<String>,
}

pub async fn smart_search(
    State(state): State<AppState>,
    Json(body): Json<SearchReq>,
) -> Json<serde_json::Value> {
    let limit = body.limit.unwrap_or(10);
    let mode = body.mode.as_deref().unwrap_or("hybrid");
    let project = body.project.as_deref();

    let results = match mode {
        "text" => crate::search::search_text(&state.db, &body.query, limit, project).await,
        "semantic" => {
            crate::search::search_semantic(&state.db, &state.embedder, &body.query, limit).await
        }
        _ => {
            let cfg = crate::search::SearchConfig {
                skip_graph: true,
                ..Default::default()
            };
            crate::search::search_hybrid_with_config(
                &state.db,
                &state.embedder,
                &body.query,
                limit,
                project,
                cfg,
            )
            .await
        }
    };

    match results {
        Ok(r) => Json(serde_json::json!({"results": r, "count": r.len()})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

pub async fn search_agentic(
    State(state): State<AppState>,
    Json(body): Json<SearchReq>,
) -> Json<serde_json::Value> {
    let limit = body.limit.unwrap_or(10);
    let project = body.project.as_deref();

    let results =
        crate::search::search_hybrid(&state.db, &state.embedder, &body.query, limit, project).await;

    match results {
        Ok(r) => {
            // Track recalled memories in the run's context trail
            if let Some(ref sid) = body.session_id {
                if let Ok(Some(run_id)) = crate::run::find_open(&state.db, sid).await {
                    let mem_hits: Vec<(surrealdb::types::RecordId, f64)> = r
                        .iter()
                        .filter(|sr| sr.obs_type.starts_with("memory:"))
                        .filter_map(|sr| Some((sr.id.clone()?, sr.score.unwrap_or(0.0))))
                        .collect();
                    if !mem_hits.is_empty() {
                        let mem_ids: Vec<_> = mem_hits.iter().map(|(id, _)| id.clone()).collect();
                        let _ = crate::run::append_recalled(&state.db, &run_id, &mem_ids).await;
                        for (mid, score) in &mem_hits {
                            let _ = crate::link::upsert_edge(
                                &state.db, mid, &run_id, "informed", "system", *score,
                            )
                            .await;
                        }
                    }
                }
            }
            Json(serde_json::json!({"results": r, "count": r.len()}))
        }
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Remember ---

#[derive(Deserialize)]
pub struct RememberReq {
    pub title: String,
    pub content: String,
    pub category: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub files: Option<Vec<String>>,
    pub project: Option<String>,
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: Option<String>,
}

pub async fn remember(
    State(state): State<AppState>,
    Json(body): Json<RememberReq>,
) -> Json<serde_json::Value> {
    let category = body.category.as_deref().unwrap_or("fact");
    let keywords = body.keywords.unwrap_or_default();
    let files = body.files.unwrap_or_default();
    let project = body.project.as_deref().unwrap_or("global");

    let save_result = crate::remember::save(
        &state.db,
        &state.embedder,
        project,
        category,
        &body.title,
        &body.content,
        &keywords,
        &files,
        body.session_id.as_deref(),
    )
    .await;

    match save_result {
        Ok(title) => {
            // Opt-in Memory Evolution fires in the background after the write commits.
            if state.llm_evolve {
                if let Some(ollama) = state.ollama.clone() {
                    let db = state.db.clone();
                    let probe_title = title.clone();
                    let probe_project = project.to_string();
                    tokio::spawn(async move {
                        // Look up the freshly-created memory id by title+project+latest-created.
                        let mut resp = match db
                            .query(
                                "SELECT id, created_at FROM memory \
                                 WHERE title = $title AND project = $project \
                                 ORDER BY created_at DESC LIMIT 1",
                            )
                            .bind(("title", probe_title))
                            .bind(("project", probe_project))
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!("evolve: id lookup failed: {e}");
                                return;
                            }
                        };
                        #[derive(
                            serde::Deserialize,
                            serde::Serialize,
                            surrealdb::types::SurrealValue,
                            Debug,
                        )]
                        struct Row {
                            id: Option<surrealdb::types::RecordId>,
                        }
                        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
                        if let Some(id) = rows.into_iter().next().and_then(|r| r.id) {
                            if let Err(e) = crate::evolve::evolve_one(&db, &ollama, &id).await {
                                tracing::warn!("evolve failed for {id:?}: {e}");
                            }
                        }
                    });
                }
            }
            Json(serde_json::json!({"status": "ok", "title": title}))
        }
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
    /// Optional query to bias retrieval (e.g. the user's prompt, pre-compact
    /// summary, or recent observation titles). When omitted, the server
    /// synthesises one from recent project activity.
    pub query: Option<String>,
}

pub async fn context(
    State(state): State<AppState>,
    Json(body): Json<ContextReq>,
) -> Json<serde_json::Value> {
    let budget = body.token_budget.unwrap_or(state.token_budget);
    let result = crate::context::generate_context_with_query(
        &state.db,
        Some(&state.embedder),
        &body.project,
        body.query.as_deref(),
        budget,
    )
    .await;
    match result {
        Ok(ctx) => Json(serde_json::json!({"context": ctx})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Runs (Phase 4) ---

#[derive(Deserialize)]
pub struct RunsReq {
    pub query: String,
    pub project: Option<String>,
    pub limit: Option<usize>,
}

pub async fn runs_search(
    State(state): State<AppState>,
    Json(body): Json<RunsReq>,
) -> Json<serde_json::Value> {
    let limit = body.limit.unwrap_or(10);
    match crate::run::search(&state.db, body.project.as_deref(), &body.query, limit).await {
        Ok(rows) => Json(serde_json::json!({"runs": rows, "count": rows.len()})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

/// GET /api/v1/agent/runs/{id} - get run with its observations
pub async fn run_detail(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let run_rid = if id.starts_with("run:") {
        id.clone()
    } else {
        format!("run:{id}")
    };

    // Get the run
    let mut resp = match state
        .db
        .query("SELECT * FROM type::record($rid)")
        .bind(("rid", run_rid.clone()))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
    };
    let runs: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let Some(run) = runs.into_iter().next() else {
        return Json(serde_json::json!({"error": "run not found"}));
    };

    // Get the observations for this run
    let mut obs_resp = match state
        .db
        .query(
            "SELECT * FROM observation WHERE id IN (SELECT VALUE observation_ids FROM type::record($rid))[0] ORDER BY timestamp ASC"
        )
        .bind(("rid", run_rid))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
    };
    let observations: Vec<serde_json::Value> = obs_resp.take(0).unwrap_or_default();

    Json(serde_json::json!({
        "run": run,
        "observations": observations
    }))
}

// --- Observations search (raw events only) ---

#[derive(Deserialize)]
pub struct ObservationsReq {
    pub query: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub limit: Option<usize>,
}

pub async fn observations_search(
    State(state): State<AppState>,
    Query(params): Query<ObservationsReq>,
) -> Json<serde_json::Value> {
    let limit = params.limit.unwrap_or(100);
    let query = params.query.as_deref().unwrap_or("*");

    // Build query based on filters
    let mut sql = String::from("SELECT * FROM observation");
    let mut conditions = Vec::new();

    if let Some(ref sid) = params.session_id {
        conditions.push(format!("session_id = type::record('session:{}')", sid));
    }

    if !query.is_empty() && query != "*" {
        // BM25 search on title and narrative
        conditions.push(format!("(title @@ $q OR narrative @@ $q)"));
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(&format!(" ORDER BY timestamp DESC LIMIT {limit}"));

    let mut resp = if query != "*" && !query.is_empty() {
        match state.db.query(&sql).bind(("q", query)).await {
            Ok(r) => r,
            Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
        }
    } else {
        match state.db.query(&sql).await {
            Ok(r) => r,
            Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
        }
    };

    let observations: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Json(serde_json::json!({"observations": observations, "count": observations.len()}))
}

// --- Memories search (memory table only) ---

#[derive(Deserialize)]
pub struct MemoriesReq {
    pub query: Option<String>,
    pub project: Option<String>,
    pub category: Option<String>,
    pub limit: Option<usize>,
}

pub async fn memories_search(
    State(state): State<AppState>,
    Query(params): Query<MemoriesReq>,
) -> Json<serde_json::Value> {
    let limit = params.limit.unwrap_or(50);
    let query = params.query.as_deref().unwrap_or("*");

    // Build query with filters
    let mut conditions = vec!["is_latest = true".to_string()];

    if let Some(ref project) = params.project {
        conditions.push(format!("(project = '{}' OR project = 'global')", project));
    }

    if let Some(ref category) = params.category {
        conditions.push(format!("category = '{}'", category));
    }

    let where_clause = conditions.join(" AND ");

    let sql = if query.is_empty() || query == "*" {
        format!("SELECT * FROM memory WHERE {where_clause} ORDER BY created_at DESC LIMIT {limit}")
    } else {
        format!(
            "SELECT *, search::score(1) + search::score(2) AS _score FROM memory \
             WHERE {where_clause} AND (title @1@ $q OR content @2@ $q) \
             ORDER BY _score DESC LIMIT {limit}"
        )
    };

    let mut resp = if query != "*" && !query.is_empty() {
        match state.db.query(&sql).bind(("q", query)).await {
            Ok(r) => r,
            Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
        }
    } else {
        match state.db.query(&sql).await {
            Ok(r) => r,
            Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
        }
    };

    let memories: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Json(serde_json::json!({"memories": memories, "count": memories.len()}))
}

// --- Core memory (always-on per-project block) ---

pub async fn core_get(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let project = params
        .get("project")
        .map(|s| s.as_str())
        .unwrap_or("global");
    match crate::core_mem::get(&state.db, project).await {
        Ok(row) => Json(serde_json::to_value(row).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

#[derive(Deserialize)]
pub struct CoreEditReq {
    pub project: String,
    pub field: String, // identity | goals | invariants | watchlist
    pub op: String,    // set | add | remove
    pub value: String,
}

pub async fn core_edit(
    State(state): State<AppState>,
    Json(body): Json<CoreEditReq>,
) -> Json<serde_json::Value> {
    match crate::core_mem::edit(&state.db, &body.project, &body.field, &body.op, &body.value).await
    {
        Ok(row) => Json(serde_json::to_value(row).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

pub async fn core_get_by_project(
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match crate::core_mem::get(&state.db, &project).await {
        Ok(row) => Json(serde_json::to_value(row).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

pub async fn core_edit_by_project(
    State(state): State<AppState>,
    axum::extract::Path(project): axum::extract::Path<String>,
    Json(body): Json<CoreEditReq>,
) -> Json<serde_json::Value> {
    match crate::core_mem::edit(&state.db, &project, &body.field, &body.op, &body.value).await {
        Ok(row) => Json(serde_json::to_value(row).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

pub async fn evolve_by_id(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let Some(ollama) = state.ollama.as_ref() else {
        return Json(serde_json::json!({
            "error": "Memory evolution requires OLLAMA_URL to be configured"
        }));
    };

    let memory_id = if id.starts_with("memory:") {
        id.clone()
    } else {
        format!("memory:{id}")
    };

    let mut resp = match state
        .db
        .query("SELECT id FROM type::record($id)")
        .bind(("id", memory_id))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
    };

    #[derive(serde::Deserialize, serde::Serialize, surrealdb::types::SurrealValue, Debug)]
    struct Row {
        id: Option<surrealdb::types::RecordId>,
    }
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    let Some(rid) = rows.into_iter().next().and_then(|r| r.id) else {
        return Json(serde_json::json!({"error": "memory not found"}));
    };

    match crate::evolve::evolve_one(&state.db, ollama, &rid).await {
        Ok(report) => Json(serde_json::to_value(report).unwrap_or_default()),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

pub async fn memory_links(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let memory_id = if id.starts_with("memory:") {
        id.clone()
    } else {
        format!("memory:{id}")
    };

    let mut resp = match state
        .db
        .query(
            "SELECT out.id AS id, out.title AS title, out.category AS category, \
             relation, score, via FROM edge WHERE in = type::record($id)",
        )
        .bind(("id", memory_id))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
    };

    let links: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Json(serde_json::json!({"links": links, "count": links.len()}))
}

// --- Trace (graph traversal) ---

#[derive(Deserialize)]
pub struct TraceReq {
    pub id: String,
    pub direction: Option<String>,
    pub relations: Option<Vec<String>>,
    pub max_hops: Option<usize>,
}

pub async fn trace_graph(
    State(state): State<AppState>,
    Json(body): Json<TraceReq>,
) -> Json<serde_json::Value> {
    let direction = body.direction.as_deref().unwrap_or("both");
    let max_hops = body.max_hops.unwrap_or(2);
    match crate::trace::trace(&state.db, &body.id, direction, body.relations, max_hops).await {
        Ok(result) => Json(serde_json::to_value(result).unwrap_or_default()),
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

// --- Evolve (opt-in Memory Evolution) ---

#[derive(Deserialize)]
pub struct EvolveReq {
    pub memory_id: String,
}

pub async fn evolve(
    State(state): State<AppState>,
    Json(body): Json<EvolveReq>,
) -> Json<serde_json::Value> {
    let Some(ollama) = state.ollama.as_ref() else {
        return Json(serde_json::json!({
            "error": "HIFZ_LLM_EVOLVE requires OLLAMA_URL to be configured"
        }));
    };

    // Resolve id string into a RecordId via the DB.
    let mut resp = match state
        .db
        .query("SELECT id FROM type::record($id)")
        .bind(("id", body.memory_id.clone()))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
    };

    #[derive(serde::Deserialize, serde::Serialize, surrealdb::types::SurrealValue, Debug)]
    struct Row {
        id: Option<surrealdb::types::RecordId>,
    }
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    let Some(rid) = rows.into_iter().next().and_then(|r| r.id) else {
        return Json(serde_json::json!({"error": "memory not found"}));
    };

    match crate::evolve::evolve_one(&state.db, ollama, &rid).await {
        Ok(report) => Json(serde_json::to_value(report).unwrap_or_default()),
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
        .query("SELECT * FROM memory")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let semantic: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM semantic_memory")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let procedural: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM procedural_memory")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let runs: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM run ORDER BY started_at DESC")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let commits: Vec<serde_json::Value> = state
        .db
        .query("SELECT * FROM commit ORDER BY timestamp DESC")
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
        "runs": runs,
        "commits": commits,
    }))
}

// --- Commits (git tracking) ---

#[derive(Deserialize)]
pub struct CommitReq {
    pub sha: String,
    pub message: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub branch: String,
    pub project: String,
    #[serde(default)]
    pub files_changed: Vec<String>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    #[serde(default)]
    pub is_amend: bool,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default = "default_timestamp")]
    pub timestamp: String,
}

fn default_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub async fn commit(
    State(state): State<AppState>,
    Json(body): Json<CommitReq>,
) -> Json<serde_json::Value> {
    let session_rid = if let Some(ref sid) = body.session_id {
        crate::run::resolve_session(&state.db, sid).await
    } else {
        None
    };

    let run_rid = if let Some(ref sid) = body.session_id {
        crate::run::find_open(&state.db, sid).await.ok().flatten()
    } else {
        None
    };

    let data = crate::commit::CommitData {
        sha: body.sha,
        message: body.message,
        author: body.author,
        branch: body.branch,
        project: body.project,
        files_changed: body.files_changed,
        insertions: body.insertions,
        deletions: body.deletions,
        is_amend: body.is_amend,
        timestamp: body.timestamp,
    };

    match crate::commit::record_commit(
        &state.db,
        data,
        session_rid,
        run_rid,
        state.git_path.as_deref(),
    )
    .await
    {
        Ok(Some(_id)) => Json(serde_json::json!({"status": "ok"})),
        Ok(None) => Json(serde_json::json!({"status": "duplicate"})),
        Err(e) => Json(serde_json::json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn commits_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let project = params.get("project").map(|s| s.as_str()).unwrap_or("");
    let branch = params.get("branch").map(|s| s.as_str());
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let sha = params.get("sha").map(|s| s.as_str());

    if let Some(sha) = sha {
        let mut resp = state
            .db
            .query("SELECT * FROM commit WHERE sha = $sha LIMIT 1")
            .bind(("sha", sha.to_string()))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        return Json(serde_json::json!({"commits": rows}));
    }

    let session_id = params.get("session_id").map(|s| s.as_str());

    let mut conditions = Vec::new();
    if !project.is_empty() {
        conditions.push("project = $project");
    }
    if branch.is_some() {
        conditions.push("branch = $branch");
    }
    if session_id.is_some() {
        conditions.push("session_id = type::record($session_id)");
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let sql = format!("SELECT * FROM commit{where_clause} ORDER BY created_at DESC LIMIT {limit}");

    let mut q = state.db.query(&sql);
    if !project.is_empty() {
        q = q.bind(("project", project.to_string()));
    }
    if let Some(b) = branch {
        q = q.bind(("branch", b.to_string()));
    }
    if let Some(sid) = session_id {
        let full_sid = if sid.starts_with("session:") {
            sid.to_string()
        } else {
            format!("session:{sid}")
        };
        q = q.bind(("session_id", full_sid));
    }
    let mut resp = q.await.unwrap();
    let commits: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Json(serde_json::json!({"commits": commits}))
}

pub async fn commit_diff(
    State(state): State<AppState>,
    axum::extract::Path(sha): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    // Look up the commit to get the project path
    let mut resp = match state
        .db
        .query("SELECT project FROM commit WHERE sha = $sha LIMIT 1")
        .bind(("sha", sha.clone()))
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": e.to_string()})),
    };

    #[derive(Debug, SurrealValue)]
    struct ProjectRow {
        project: Option<String>,
    }
    let rows: Vec<ProjectRow> = resp.take(0).unwrap_or_default();
    let project = match rows.into_iter().next().and_then(|r| r.project) {
        Some(p) => p,
        None => return Json(serde_json::json!({"error": "commit not found"})),
    };

    let Some(ref git) = state.git_path else {
        return Json(serde_json::json!({"error": "git not available"}));
    };

    let output = std::process::Command::new(git)
        .args(["show", "--stat", "--patch", "--format=", &sha])
        .current_dir(&project)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let diff = String::from_utf8_lossy(&o.stdout).to_string();
            Json(serde_json::json!({"sha": sha, "diff": diff}))
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).to_string();
            Json(serde_json::json!({"error": err}))
        }
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

// --- Plans ---

pub async fn plan_upsert(
    State(state): State<AppState>,
    Json(body): Json<crate::plan::PlanUpsertRequest>,
) -> Json<serde_json::Value> {
    match crate::plan::upsert(&state.db, &body).await {
        Ok(plan) => Json(serde_json::json!({
            "status": "ok",
            "plan": serde_json::to_value(plan).unwrap_or_default()
        })),
        Err(e) => Json(serde_json::json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn plans_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let project = params.get("project").map(|s| s.as_str()).unwrap_or("");
    let status = params.get("status").map(|s| s.as_str());
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    match crate::plan::list(&state.db, project, status, limit).await {
        Ok(plans) => Json(serde_json::json!({"plans": plans})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

pub async fn plan_current(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let project = params.get("project").map(|s| s.as_str()).unwrap_or("");
    match crate::plan::get_active(&state.db, project).await {
        Ok(Some(plan)) => Json(serde_json::to_value(plan).unwrap_or_default()),
        Ok(None) => Json(serde_json::json!(null)),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

#[derive(Deserialize)]
pub struct PlanCompleteReq {
    pub commit_id: Option<String>,
}

pub async fn plan_complete(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<PlanCompleteReq>,
) -> Json<serde_json::Value> {
    match crate::plan::complete(&state.db, &id, body.commit_id.as_deref()).await {
        Ok(()) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

pub async fn plan_abandon(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    match crate::plan::abandon(&state.db, &id).await {
        Ok(()) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
}

#[derive(Deserialize)]
pub struct PlanActivateReq {
    pub project: String,
    pub plan_id: Option<String>,
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: Option<String>,
}

pub async fn plan_activate(
    State(state): State<AppState>,
    Json(body): Json<PlanActivateReq>,
) -> Json<serde_json::Value> {
    match crate::plan::activate(
        &state.db,
        &body.project,
        body.plan_id.as_deref(),
        body.session_id.as_deref(),
    )
    .await
    {
        Ok(Some(plan)) => Json(serde_json::json!({
            "status": "ok",
            "plan": serde_json::to_value(plan).unwrap_or_default()
        })),
        Ok(None) => Json(serde_json::json!({"status": "no_active_plan"})),
        Err(e) => Json(serde_json::json!({"error": e.to_string()})),
    }
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
