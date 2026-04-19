pub mod api;

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::Router;
use surrealdb::Surreal;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

use crate::db::Db;
use crate::dedup::DedupMap;
use crate::embed::Embedder;
use crate::ollama::OllamaClient;

#[derive(Clone)]
pub struct AppState {
    pub db: Surreal<Db>,
    pub embedder: Arc<Embedder>,
    pub ollama: Option<Arc<OllamaClient>>,
    pub dedup: Arc<DedupMap>,
    pub auto_compress: bool,
    pub token_budget: usize,
    pub llm_evolve: bool,
    pub started_at: Instant,
}

pub async fn serve(
    db: Surreal<Db>,
    port: u16,
    embedder: Arc<Embedder>,
    ollama: Option<Arc<OllamaClient>>,
    auto_compress: bool,
    token_budget: usize,
    llm_evolve: bool,
) -> Result<()> {
    let state = AppState {
        db,
        embedder,
        ollama,
        dedup: Arc::new(DedupMap::new()),
        auto_compress,
        token_budget,
        llm_evolve,
        started_at: Instant::now(),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Health
        .route("/hifz/health", axum::routing::get(api::health))
        .route("/hifz/livez", axum::routing::get(api::livez))
        // Session management
        .route(
            "/hifz/session/start",
            axum::routing::post(api::session_start),
        )
        .route("/hifz/session/end", axum::routing::post(api::session_end))
        .route("/hifz/sessions", axum::routing::get(api::sessions_list))
        // Observation capture
        .route("/hifz/observe", axum::routing::post(api::observe))
        // Search
        .route("/hifz/smart-search", axum::routing::post(api::smart_search))
        .route("/hifz/search", axum::routing::post(api::smart_search))
        // Memory management
        .route("/hifz/remember", axum::routing::post(api::remember))
        .route("/hifz/forget", axum::routing::post(api::forget))
        // Context
        .route("/hifz/context", axum::routing::post(api::context))
        // Core memory (always-on per-project block)
        .route("/hifz/core", axum::routing::get(api::core_get))
        .route("/hifz/core/edit", axum::routing::post(api::core_edit))
        // Episodes (task-scoped trajectories)
        .route("/hifz/episodes", axum::routing::post(api::episodes_search))
        // Digest (project intelligence)
        .route("/hifz/digest", axum::routing::get(api::digest))
        // Forget GC (garbage collection)
        .route("/hifz/forget-gc", axum::routing::post(api::forget_gc))
        // Consolidation
        .route("/hifz/consolidate", axum::routing::post(api::consolidate))
        // Memory Evolution (opt-in LLM)
        .route("/hifz/evolve", axum::routing::post(api::evolve))
        // Timeline
        .route("/hifz/timeline", axum::routing::get(api::timeline))
        // Export
        .route("/hifz/export", axum::routing::get(api::export))
        .layer(cors)
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    tracing::info!("REST API listening on http://{addr}/hifz/*");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
