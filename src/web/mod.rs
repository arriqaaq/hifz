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
        .route("/consolidate", axum::routing::post(api::consolidate))
        .route("/forget-gc", axum::routing::post(api::forget_gc))
        .route("/export", axum::routing::get(api::export));

    // --- Agent Pipeline API (sessions, observations, runs, git, plans) ---
    let agent_api = Router::new()
        .route(
            "/sessions",
            axum::routing::post(api::session_start).get(api::sessions_list),
        )
        .route("/sessions/end", axum::routing::post(api::session_end))
        .route("/sessions/{id}", axum::routing::get(api::session_get))
        .route("/observe", axum::routing::post(api::observe))
        .route(
            "/observations",
            axum::routing::get(api::observations_search),
        )
        .route("/timeline", axum::routing::get(api::timeline))
        .route("/runs", axum::routing::post(api::runs_search))
        .route("/runs/{id}", axum::routing::get(api::run_detail))
        .route(
            "/commits",
            axum::routing::post(api::commit).get(api::commits_list),
        )
        .route("/commits/{sha}/diff", axum::routing::get(api::commit_diff))
        .route(
            "/plans",
            axum::routing::post(api::plan_upsert).get(api::plans_list),
        )
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
        .route("/digest", axum::routing::get(api::digest));

    let api = Router::new()
        .nest("/api/v1", core_api)
        .nest("/api/v1/agent", agent_api)
        .with_state(state);

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
