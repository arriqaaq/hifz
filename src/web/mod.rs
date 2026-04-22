pub mod api;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use axum::Router;
use dashmap::DashMap;
use surrealdb::Surreal;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

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
    pub git_path: Option<PathBuf>,
    pub git_repo_cache: Arc<DashMap<String, bool>>,
}

impl AppState {
    pub fn is_git_repo(&self, project: &str) -> bool {
        if let Some(cached) = self.git_repo_cache.get(project) {
            return *cached;
        }
        let is_repo = self
            .git_path
            .as_ref()
            .map(|git| {
                std::process::Command::new(git)
                    .args(["rev-parse", "--git-dir"])
                    .current_dir(project)
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !is_repo {
            tracing::debug!("{project} is not a git repository — commit enrichment skipped");
        }
        self.git_repo_cache.insert(project.to_string(), is_repo);
        is_repo
    }
}

pub async fn serve(
    db: Surreal<Db>,
    port: u16,
    embedder: Arc<Embedder>,
    ollama: Option<Arc<OllamaClient>>,
    auto_compress: bool,
    token_budget: usize,
    llm_evolve: bool,
    git_path: Option<PathBuf>,
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
        git_path,
        git_repo_cache: Arc::new(DashMap::new()),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
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
        .route("/hifz/session/{id}", axum::routing::get(api::session_get))
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
        // Runs (task-scoped trajectories)
        .route("/hifz/runs", axum::routing::post(api::runs_search))
        .route("/hifz/run/{id}", axum::routing::get(api::run_detail))
        // Observations (raw events)
        .route(
            "/hifz/observations",
            axum::routing::get(api::observations_search),
        )
        // Memories (hifz table only)
        .route("/hifz/memories", axum::routing::get(api::memories_search))
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
        // Commits (git tracking)
        .route("/hifz/commit", axum::routing::post(api::commit))
        .route("/hifz/commits", axum::routing::get(api::commits_list))
        .route(
            "/hifz/commits/{sha}/diff",
            axum::routing::get(api::commit_diff),
        )
        // Plans (first-class plan tracking)
        .route("/hifz/plan", axum::routing::post(api::plan_upsert))
        .route("/hifz/plans", axum::routing::get(api::plans_list))
        .route("/hifz/plan/current", axum::routing::get(api::plan_current))
        .route(
            "/hifz/plan/activate",
            axum::routing::post(api::plan_activate),
        )
        .route(
            "/hifz/plan/{id}/complete",
            axum::routing::post(api::plan_complete),
        )
        .route(
            "/hifz/plan/{id}/abandon",
            axum::routing::post(api::plan_abandon),
        )
        // Export
        .route("/hifz/export", axum::routing::get(api::export))
        .with_state(state);

    // Serve frontend static files with SPA fallback
    let spa_fallback = ServeFile::new("website/build/index.html");
    let static_files = ServeDir::new("website/build").not_found_service(spa_fallback);

    let app = api.fallback_service(static_files).layer(cors);

    let addr = format!("127.0.0.1:{port}");
    tracing::info!("REST API listening on http://{addr}/hifz/*");

    let listener = TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
