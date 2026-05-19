//! REST handlers — pure JSON marshalling around `Hifz` library methods.
//!
//! Every business-logic line lives in a core module (`session.rs`,
//! `observe.rs`, etc.) and is dispatched through a `Hifz` method. Handlers
//! here just parse JSON, call the method, and stringify the result.

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;
use surrealdb::types::SurrealValue;

use crate::models::{CodeGcReq, CodeIndexReq, CodeLinkReq, CodeLinkSymReq, CodeSearchReq};
use crate::models::{
    CommitsReq, ContextReq, CoreEditReq, ExportReq, HookPayload, MemoriesReq, ObservationsReq,
    PlanActivateReq, PlansListReq, RememberReq, RunsReq, SearchReq, SessionStartReq, TimelineReq,
    TraceReq,
};
use crate::web::AppState;
use crate::web::error::{ApiError, ApiResult, AppJson};

// -----------------------------------------------------------------------
// Helper macros to keep the dispatch boilerplate uniform.
// -----------------------------------------------------------------------

/// `anyhow::Result<T>` -> `ApiResult` — pass the value through as JSON, or
/// surface the error with a real HTTP status (`ApiError::from` classifies
/// `HifzError::NotFound`→404 / `InvalidInput`→400 / else→500).
fn json_or_err<T: serde::Serialize>(r: anyhow::Result<T>) -> ApiResult {
    match r {
        Ok(v) => Ok(Json(serde_json::to_value(v).unwrap_or_default())),
        Err(e) => Err(ApiError::from(e)),
    }
}

// -----------------------------------------------------------------------
// Health
// -----------------------------------------------------------------------

pub async fn health(State(state): State<AppState>) -> ApiResult {
    json_or_err(state.health().await)
}

pub async fn livez() -> &'static str {
    "ok"
}

// -----------------------------------------------------------------------
// Sessions
// -----------------------------------------------------------------------

pub async fn session_start(
    State(state): State<AppState>,
    AppJson(body): AppJson<SessionStartReq>,
) -> ApiResult {
    json_or_err(state.session_start(body).await)
}

#[derive(Deserialize)]
pub struct SessionEndReq {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

pub async fn session_end(
    State(state): State<AppState>,
    AppJson(body): AppJson<SessionEndReq>,
) -> ApiResult {
    json_or_err(state.session_end(&body.session_id).await)
}

pub async fn session_get(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.session_get(&id).await)
}

/// Phase 3.1: project-scoped warmup digest at session start.
/// `?project=foo&top_n=15` query params override session-row's project.
pub async fn session_warmup(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let project = params.get("project").map(String::as_str);
    let top_n = params
        .get("top_n")
        .or_else(|| params.get("limit"))
        .and_then(|v| v.parse::<usize>().ok());
    json_or_err(state.session_warmup(&id, project, top_n).await)
}

pub async fn sessions_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    json_or_err(state.sessions_list(limit).await)
}

// -----------------------------------------------------------------------
// Observe
// -----------------------------------------------------------------------

pub async fn observe(
    State(state): State<AppState>,
    AppJson(payload): AppJson<HookPayload>,
) -> ApiResult {
    match state.observe(payload).await {
        Ok(Some(obs_id)) => Ok(Json(serde_json::json!({"status": "ok", "obs_id": obs_id}))),
        Ok(None) => Ok(Json(serde_json::json!({"status": "duplicate"}))),
        Err(e) => Err(ApiError::from(e)),
    }
}

// -----------------------------------------------------------------------
// Search & context
// -----------------------------------------------------------------------

pub async fn smart_search(
    State(state): State<AppState>,
    AppJson(body): AppJson<SearchReq>,
) -> ApiResult {
    json_or_err(state.smart_search(body).await)
}

pub async fn search_agentic(
    State(state): State<AppState>,
    AppJson(body): AppJson<SearchReq>,
) -> ApiResult {
    // Note: search_agentic preserves the legacy run-linkage side effect:
    // when sessionId is supplied AND a run is open AND any results are
    // memories, append them to the run's recall trail and create
    // `memory --informed--> run` edges. This belongs at the handler layer
    // because it's an HTTP-shape concern (sessionId comes from the request).
    let limit = body.limit.unwrap_or(10);
    let project = body.project.as_deref();

    let results =
        match crate::search::search_hybrid(&state.db, &state.embedder, &body.query, limit, project)
            .await
        {
            Ok(r) => r,
            Err(e) => return Err(ApiError::from(e)),
        };

    if let Some(ref sid) = body.session_id
        && let Ok(Some(run_id)) = crate::run::find_open(&state.db, sid).await
    {
        let mem_hits: Vec<(surrealdb::types::RecordId, f64)> = results
            .iter()
            .filter(|sr| sr.obs_type.starts_with("memory:"))
            .filter_map(|sr| Some((sr.id.clone()?, sr.score.unwrap_or(0.0))))
            .collect();
        if !mem_hits.is_empty() {
            let mem_ids: Vec<_> = mem_hits.iter().map(|(id, _)| id.clone()).collect();
            let _ = crate::run::append_recalled(&state.db, &run_id, &mem_ids).await;
            for (mid, score) in &mem_hits {
                let reason = format!("recalled by search rank {score:.3}");
                let _ = crate::link::upsert_edge(
                    &state.db,
                    mid,
                    &run_id,
                    "informed_by",
                    "system",
                    *score,
                    Some(&reason),
                )
                .await;
            }
        }
    }

    let count = results.len();
    Ok(Json(
        serde_json::json!({"results": results, "count": count}),
    ))
}

pub async fn context(
    State(state): State<AppState>,
    AppJson(body): AppJson<ContextReq>,
) -> ApiResult {
    match state.context(body).await {
        Ok(s) => Ok(Json(serde_json::json!({"context": s}))),
        Err(e) => Err(ApiError::from(e)),
    }
}

// -----------------------------------------------------------------------
// Memory
// -----------------------------------------------------------------------

pub async fn remember(
    State(state): State<AppState>,
    AppJson(body): AppJson<RememberReq>,
) -> ApiResult {
    let llm_evolve = state.llm_evolve;
    let ollama_clone = state.ollama.clone();
    let db_clone = state.db.clone();
    let project_for_evolve = body.project.clone().unwrap_or_else(|| "global".to_string());

    match state.remember(body).await {
        Ok(v) => {
            // Side-effect: opt-in Memory Evolution after the write commits.
            // Lifted from the legacy handler. Looks up the freshly-created
            // memory by title+project, then calls evolve_one in the background.
            if llm_evolve && let Some(ollama) = ollama_clone {
                let probe_title = v
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                if !probe_title.is_empty() {
                    tokio::spawn(async move {
                        let mut resp = match db_clone
                            .query(
                                "SELECT id, created_at FROM memory \
                                     WHERE title = $title AND project = $project \
                                     ORDER BY created_at DESC LIMIT 1",
                            )
                            .bind(("title", probe_title))
                            .bind(("project", project_for_evolve))
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
                        if let Some(id) = rows.into_iter().next().and_then(|r| r.id)
                            && let Err(e) = crate::evolve::evolve_one(&db_clone, &ollama, &id).await
                        {
                            tracing::warn!("evolve failed for {id:?}: {e}");
                        }
                    });
                }
            }
            Ok(Json(v))
        }
        Err(e) => Err(ApiError::from(e)),
    }
}

#[derive(Deserialize)]
pub struct ForgetReq {
    pub id: String,
}

pub async fn forget(State(state): State<AppState>, AppJson(body): AppJson<ForgetReq>) -> ApiResult {
    json_or_err(state.forget(&body.id).await)
}

pub async fn memories_search(
    State(state): State<AppState>,
    Query(params): Query<MemoriesReq>,
) -> ApiResult {
    json_or_err(state.memories_search(params).await)
}

pub async fn memory_links(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.memory_links(&id).await)
}

pub async fn memory_view(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.memory_view(&id).await)
}

/// Static render theme tokens (tone → hex). The web UI maps these to CSS
/// custom properties so terminal and browser share one palette.
pub async fn render_tokens() -> Json<serde_json::Value> {
    Json(memdiff::theme::tokens())
}

pub async fn replays_list(State(state): State<AppState>) -> ApiResult {
    json_or_err(state.replays_list().await)
}

pub async fn replay_get(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.replay_get(&id).await)
}

/// Phase 5: typed graph walk from a memory. Query params:
/// `?relations=related,elaborates&max_hops=2`.
pub async fn memory_neighbors(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let relations = params.get("relations").map(|s| {
        s.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
    });
    let max_hops = params.get("max_hops").and_then(|v| v.parse::<usize>().ok());
    json_or_err(state.memory_neighbors(&id, relations, max_hops).await)
}

/// Phase 5: chronological digest of recent activity for a project, grouped
/// by category. `?days=N` (default 30).
pub async fn project_digest(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let days = params.get("days").and_then(|v| v.parse::<i64>().ok());
    json_or_err(state.project_digest(&project, days).await)
}

/// Phase 5: project accumulator rollup (latest plan, decisions, conventions,
/// open bugs, gotchas, failure patterns, recent lessons).
pub async fn project_accumulators(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult {
    json_or_err(state.project_accumulators(&project).await)
}

/// Phase 4.5: incoming edges for a memory. `?relation=foo` to filter.
pub async fn memory_backlinks(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let relation = params.get("relation").map(String::as_str);
    json_or_err(state.memory_backlinks(&id, relation).await)
}

/// Phase 4.4: render a memory as frontmatter-rich markdown.
/// Returns `text/markdown` with the rendered body — NOT JSON-wrapped, so
/// callers can pipe this directly to a file or `$EDITOR`.
pub async fn memory_markdown_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    match state.memory_markdown_get(&id).await {
        Ok(s) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/markdown; charset=utf-8",
            )],
            s,
        )
            .into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

/// Phase 4.4: parse an edited markdown blob and write a NEW memory version
/// that supersedes the old one. Body is plain text (the markdown), not JSON.
pub async fn memory_markdown_put(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: String,
) -> ApiResult {
    json_or_err(state.memory_markdown_put(&id, &body).await)
}

pub async fn evolve_by_id(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.evolve(&id).await)
}

/// Body-based variant — keeps the legacy POST `/memories/{id}/evolve` shape.
pub async fn evolve(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.evolve(&id).await)
}

// -----------------------------------------------------------------------
// Runs
// -----------------------------------------------------------------------

pub async fn runs_search(
    State(state): State<AppState>,
    AppJson(body): AppJson<RunsReq>,
) -> ApiResult {
    json_or_err(state.runs_search(body).await)
}

pub async fn run_detail(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.run_detail(&id).await)
}

pub async fn session_tree(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.session_tree(&id).await)
}

// -----------------------------------------------------------------------
// Observations
// -----------------------------------------------------------------------

pub async fn observations_search(
    State(state): State<AppState>,
    Query(params): Query<ObservationsReq>,
) -> ApiResult {
    json_or_err(state.observations_search(params).await)
}

// -----------------------------------------------------------------------
// Core memory
// -----------------------------------------------------------------------

pub async fn core_get(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let project = params
        .get("project")
        .map(|s| s.as_str())
        .unwrap_or("global");
    json_or_err(state.core_get(project).await)
}

pub async fn core_edit(
    State(state): State<AppState>,
    AppJson(body): AppJson<CoreEditReq>,
) -> ApiResult {
    let project = body.project.clone();
    json_or_err(state.core_edit(&project, body).await)
}

pub async fn core_get_by_project(
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> ApiResult {
    json_or_err(state.core_get(&project).await)
}

pub async fn core_edit_by_project(
    State(state): State<AppState>,
    Path(project): Path<String>,
    AppJson(body): AppJson<CoreEditReq>,
) -> ApiResult {
    json_or_err(state.core_edit(&project, body).await)
}

// -----------------------------------------------------------------------
// Trace (graph)
// -----------------------------------------------------------------------

pub async fn trace_graph(
    State(state): State<AppState>,
    AppJson(body): AppJson<TraceReq>,
) -> ApiResult {
    json_or_err(state.trace(body).await)
}

// -----------------------------------------------------------------------
// Project intelligence
// -----------------------------------------------------------------------

pub async fn digest(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let project = params.get("project").map(|s| s.as_str());
    json_or_err(state.digest(project).await)
}

pub async fn timeline(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let req = TimelineReq {
        session_id: params.get("session_id").cloned(),
        limit: params.get("limit").and_then(|v| v.parse().ok()),
    };
    json_or_err(state.timeline(req).await)
}

pub async fn timeline_causal(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let seed = params.get("seed").map(|s| s.as_str());
    let project = params.get("project").map(|s| s.as_str());
    let max_hops = params
        .get("max_hops")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let limit = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    json_or_err(state.timeline_causal(seed, project, max_hops, limit).await)
}

pub async fn commits_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let req = CommitsReq {
        project: params.get("project").cloned(),
        branch: params.get("branch").cloned(),
        limit: params.get("limit").and_then(|v| v.parse().ok()),
        sha: params.get("sha").cloned(),
    };
    json_or_err(state.commits_list(req).await)
}

pub async fn commit_diff(State(state): State<AppState>, Path(sha): Path<String>) -> ApiResult {
    json_or_err(state.commit_diff(&sha).await)
}

// -----------------------------------------------------------------------
// Plans
// -----------------------------------------------------------------------

pub async fn plans_list(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let req = PlansListReq {
        project: params.get("project").cloned(),
        status: params.get("status").cloned(),
        limit: params.get("limit").and_then(|v| v.parse().ok()),
    };
    json_or_err(state.plans_list(req).await)
}

pub async fn plan_current(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let project = params.get("project").map(|s| s.as_str());
    json_or_err(state.plan_current(project).await)
}

pub async fn plan_complete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    AppJson(_body): AppJson<serde_json::Value>,
) -> ApiResult {
    json_or_err(state.plan_complete(&id).await)
}

pub async fn plan_abandon(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    json_or_err(state.plan_abandon(&id).await)
}

pub async fn plan_activate(
    State(state): State<AppState>,
    AppJson(body): AppJson<PlanActivateReq>,
) -> ApiResult {
    json_or_err(state.plan_activate(body).await)
}

// -----------------------------------------------------------------------
// Maintenance
// -----------------------------------------------------------------------

pub async fn forget_gc(State(state): State<AppState>) -> ApiResult {
    json_or_err(state.forget_gc(false).await)
}

pub async fn consolidate(State(state): State<AppState>) -> ApiResult {
    json_or_err(state.consolidate().await)
}

pub async fn export(State(state): State<AppState>, Query(params): Query<ExportReq>) -> ApiResult {
    json_or_err(state.export(params).await)
}

// -----------------------------------------------------------------------
// Agent usage (generic LLM token tracking — adapter-populated)
// -----------------------------------------------------------------------

pub async fn usage_record(
    State(state): State<AppState>,
    AppJson(body): AppJson<crate::models::AgentUsageRecord>,
) -> ApiResult {
    json_or_err(state.usage_record(body).await)
}

#[derive(Deserialize)]
pub struct UsageBatchReq {
    pub records: Vec<crate::models::AgentUsageRecord>,
}

pub async fn usage_record_batch(
    State(state): State<AppState>,
    AppJson(body): AppJson<UsageBatchReq>,
) -> ApiResult {
    json_or_err(state.usage_record_batch(body.records).await)
}

pub async fn usage_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult {
    json_or_err(state.usage_for_session(&session_id).await)
}

pub async fn usage_project(
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let filters = crate::usage::ProjectFilters {
        from: params.get("from").cloned(),
        to: params.get("to").cloned(),
        model: params.get("model").cloned(),
    };
    json_or_err(state.usage_for_project(&project, filters).await)
}

/// Per-session token totals, uncapped. Optional `?project=` scopes to one
/// project; absent = every session with usage data (the `/sessions` list
/// spans projects). Returns `{ sessions: [...] }`.
pub async fn usage_sessions(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> ApiResult {
    let project = params.get("project").map(|s| s.as_str());
    match state.usage_session_totals(project).await {
        Ok(sessions) => Ok(Json(serde_json::json!({ "sessions": sessions }))),
        Err(e) => Err(ApiError::from(e)),
    }
}

// -----------------------------------------------------------------------
// Code dimension (M2+).
// -----------------------------------------------------------------------

pub async fn code_index(
    State(state): State<AppState>,
    AppJson(body): AppJson<CodeIndexReq>,
) -> ApiResult {
    json_or_err(state.code_index(body).await)
}

pub async fn code_search(
    State(state): State<AppState>,
    AppJson(body): AppJson<CodeSearchReq>,
) -> ApiResult {
    json_or_err(state.code_search(body).await)
}

pub async fn code_link(
    State(state): State<AppState>,
    AppJson(body): AppJson<CodeLinkReq>,
) -> ApiResult {
    json_or_err(state.code_link(body).await)
}

pub async fn code_link_symbol(
    State(state): State<AppState>,
    AppJson(body): AppJson<CodeLinkSymReq>,
) -> ApiResult {
    json_or_err(state.code_link_symbol(body).await)
}

pub async fn code_gc(
    State(state): State<AppState>,
    AppJson(body): AppJson<CodeGcReq>,
) -> ApiResult {
    json_or_err(state.code_gc(body).await)
}
