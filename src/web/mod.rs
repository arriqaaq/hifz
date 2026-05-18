pub mod api;
pub mod error;

use anyhow::Result;
use axum::Router;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

use crate::Hifz;

/// Axum-shared state. Aliased to `Hifz` so handlers continue to compile
/// unchanged; the rename is purely structural.
pub type AppState = Hifz;

pub async fn serve(state: Hifz, port: u16) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // atlas (feature-gated): ensure its tables exist in the shared store.
    #[cfg(feature = "atlas")]
    if let Err(e) = atlas::store::init_atlas_schema(&state.db, state.embedder.dimension()).await {
        tracing::error!("atlas schema init failed: {e}");
    }

    // --- Core Memory API (no session/hook/git dependency, scoped by project) ---
    let core_api = Router::new()
        .route("/health", axum::routing::get(api::health))
        .route("/livez", axum::routing::get(api::livez))
        .route(
            "/memories",
            axum::routing::post(api::remember)
                .get(api::memories_search)
                .delete(api::forget),
        )
        .route("/search", axum::routing::post(api::smart_search))
        .route("/search/agentic", axum::routing::post(api::search_agentic))
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
        .route("/export", axum::routing::get(api::export));

    #[cfg(feature = "code")]
    let core_api = core_api
        .route("/code/index", axum::routing::post(api::code_index))
        .route("/code/search", axum::routing::post(api::code_search))
        .route("/code/link", axum::routing::post(api::code_link))
        .route(
            "/code/link/symbol",
            axum::routing::post(api::code_link_symbol),
        )
        .route("/code/gc", axum::routing::post(api::code_gc));

    // --- Agent Pipeline API (sessions, observations, runs, git, plans) ---
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
        );

    // atlas router carries its own state (→ `Router<()>` after `with_state`);
    // build it before `state` is consumed below.
    #[cfg(feature = "atlas")]
    let atlas_router = atlas::web::router(atlas::web::AtlasState {
        db: state.db.clone(),
        embedder: state.embedder.clone(),
        project: std::env::var("ATLAS_PROJECT").unwrap_or_else(|_| "default".into()),
    });

    let api = Router::new()
        .nest("/api/v1", core_api)
        .nest("/api/v1/agent", agent_api)
        .with_state(state);

    // Both are now `Router<()>` → composes cleanly.
    #[cfg(feature = "atlas")]
    let api = api.nest("/api/v1/atlas", atlas_router);

    // Serve frontend static files with SPA fallback
    let spa_fallback = ServeFile::new("website/build/index.html");
    let static_files = ServeDir::new("website/build").not_found_service(spa_fallback);

    let app = api.fallback_service(static_files).layer(cors);

    let addr = format!("127.0.0.1:{port}");
    tracing::info!("REST API listening on http://{addr}/api/v1/*");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
