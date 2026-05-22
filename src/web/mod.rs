pub mod api;
pub mod error;

use anyhow::Result;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

use crate::Hifz;

/// Axum-shared state. Aliased to `Hifz` so handlers continue to compile
/// unchanged; the rename is purely structural.
pub type AppState = Hifz;

pub async fn serve(state: Hifz, port: u16) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // maktab (feature-gated): ensure its tables exist in the shared store.
    #[cfg(feature = "maktab")]
    if let Err(e) = maktab::store::init_maktab_schema(&state.db, state.embedder.dimension()).await {
        tracing::error!("maktab schema init failed: {e}");
    }

    // Core Memory API (no session/hook/git dependency, scoped by project)
    let core_api = Router::new()
        .route("/health", axum::routing::get(api::health))
        .route("/livez", axum::routing::get(api::livez))
        .route(
            "/memories",
            axum::routing::post(api::remember)
                .get(api::memories_search)
                .delete(api::forget),
        )
        .route("/search", axum::routing::post(api::search))
        .route("/search/session", axum::routing::post(api::search_session))
        .route("/context", axum::routing::post(api::context))
        .route(
            "/core/{project}",
            axum::routing::get(api::core_get_by_project).patch(api::core_edit_by_project),
        )
        .route(
            "/memories/{id}/evolve",
            axum::routing::post(api::evolve_by_id),
        )
        .route(
            "/memories/{id}/links",
            axum::routing::get(api::memory_links),
        )
        .route("/memories/{id}/view", axum::routing::get(api::memory_view))
        .route(
            "/memories/{id}/backlinks",
            axum::routing::get(api::memory_backlinks),
        )
        .route(
            "/memories/{id}/markdown",
            axum::routing::get(api::memory_markdown_get).put(api::memory_markdown_put),
        )
        .route(
            "/memories/{id}/neighbors",
            axum::routing::get(api::memory_neighbors),
        )
        .route(
            "/projects/{project}/digest",
            axum::routing::get(api::project_digest),
        )
        .route(
            "/projects/{project}/accumulators",
            axum::routing::get(api::project_accumulators),
        )
        .route("/trace", axum::routing::post(api::trace_graph))
        .route("/consolidate", axum::routing::post(api::consolidate))
        .route("/forget-gc", axum::routing::post(api::forget_gc))
        .route("/render/tokens", axum::routing::get(api::render_tokens))
        .route("/replays", axum::routing::get(api::replays_list))
        .route("/replays/{id}", axum::routing::get(api::replay_get))
        .route("/export", axum::routing::get(api::export));

    let core_api = core_api
        .route("/code/index", axum::routing::post(api::code_index))
        .route("/code/search", axum::routing::post(api::code_search))
        .route("/code/projects", axum::routing::get(api::code_projects))
        .route("/code/link", axum::routing::post(api::code_link))
        .route(
            "/code/link/symbol",
            axum::routing::post(api::code_link_symbol),
        )
        .route("/code/gc", axum::routing::post(api::code_gc))
        .route(
            "/code/watch",
            axum::routing::post(api::code_watch_start).delete(api::code_watch_stop),
        )
        .route("/code/watchers", axum::routing::get(api::code_watchers));

    // Agent Pipeline API (sessions, observations, runs, git, plans)
    let agent_api = Router::new()
        .route(
            "/sessions",
            axum::routing::post(api::session_start).get(api::sessions_list),
        )
        .route("/sessions/end", axum::routing::post(api::session_end))
        .route("/sessions/{id}", axum::routing::get(api::session_get))
        .route("/sessions/{id}/tree", axum::routing::get(api::session_tree))
        .route(
            "/sessions/{id}/warmup",
            axum::routing::post(api::session_warmup).get(api::session_warmup),
        )
        .route("/observe", axum::routing::post(api::observe))
        .route(
            "/observations",
            axum::routing::get(api::observations_search),
        )
        .route("/timeline", axum::routing::get(api::timeline))
        .route("/timeline/causal", axum::routing::get(api::timeline_causal))
        .route("/runs", axum::routing::post(api::runs_search))
        .route("/runs/{id}", axum::routing::get(api::run_detail))
        .route("/commits", axum::routing::get(api::commits_list))
        .route("/commits/{sha}/diff", axum::routing::get(api::commit_diff))
        .route("/plans", axum::routing::get(api::plans_list))
        .route("/plans/current", axum::routing::get(api::plan_current))
        .route("/plans/activate", axum::routing::post(api::plan_activate))
        .route(
            "/plans/{id}/complete",
            axum::routing::post(api::plan_complete),
        )
        .route(
            "/plans/{id}/abandon",
            axum::routing::post(api::plan_abandon),
        )
        .route("/digest", axum::routing::get(api::digest))
        .route("/usage", axum::routing::post(api::usage_record))
        .route("/usage/batch", axum::routing::post(api::usage_record_batch))
        .route("/usage/sessions", axum::routing::get(api::usage_sessions))
        .route(
            "/usage/session/{session_id}",
            axum::routing::get(api::usage_session),
        )
        .route(
            "/usage/project/{project}",
            axum::routing::get(api::usage_project),
        )
        // Lean memory search for agents/MCP (full content, no metadata noise;
        // UI keeps /search + /search/session).
        .route("/search", axum::routing::post(api::agent_search))
        // Lean code search for agents/MCP (token-cheap; UI keeps /code/search).
        .route(
            "/code/symbols",
            axum::routing::post(api::agent_code_symbols),
        )
        .route(
            "/code/semantic",
            axum::routing::post(api::agent_code_semantic),
        );

    // maktab router carries its own state (→ `Router<()>` after `with_state`);
    // build it before `state` is consumed below.
    #[cfg(feature = "maktab")]
    let maktab_router = maktab::web::router(maktab::web::MaktabState {
        db: state.db.clone(),
        embedder: state.embedder.clone(),
        jobs: Default::default(),
    });

    let api = Router::new()
        .nest("/api/v1", core_api)
        .nest("/api/v1/agent", agent_api)
        .with_state(state);

    // Both are now `Router<()>` → composes cleanly.
    #[cfg(feature = "maktab")]
    let api = api.nest("/api/v1/maktab", maktab_router);

    // Serve frontend static files with SPA fallback. A plain
    // `ServeFile` not_found_service returns the shell body but a 404 status,
    // so deep links / refresh on client-routed pages (/maktab, /sessions, …)
    // 404. A handler returns the SvelteKit shell with an explicit 200.
    let static_files = ServeDir::new("website/build").not_found_service(get(spa_index));

    let app = api.fallback_service(static_files).layer(cors);

    let addr = format!("127.0.0.1:{port}");
    tracing::info!("REST API listening on http://{addr}/api/v1/*");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// SPA fallback handler: serve the SvelteKit shell with an explicit **200**
/// so client-routed deep links / refresh (e.g. `/maktab`) work. Path is
/// relative to the daemon `WorkingDirectory` (the repo root).
async fn spa_index() -> impl IntoResponse {
    match tokio::fs::read("website/build/index.html").await {
        Ok(bytes) => Html(bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "frontend not built").into_response(),
    }
}
