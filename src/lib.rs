use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use dashmap::DashMap;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

pub mod commits;
pub mod compress;
pub mod config;
pub mod consolidate;
pub mod context;
pub mod core_mem;
pub mod db;
pub mod dedup;
pub mod digest;
pub mod embed;
pub mod entities;
pub mod event;
pub mod evolve;
pub mod export;
pub mod forget;
pub mod ground;
pub mod health;
pub mod link;
pub mod llm_rerank;
pub mod mcp;
pub mod models;
pub mod observe;
pub mod ollama;
pub mod plans;
pub mod prompts;
pub mod rank;
pub mod reindex;
pub mod remember;
pub mod rerank;
pub mod run;
pub mod search;
pub mod session;
pub mod timeline;
pub mod trace;
pub mod web;

use crate::db::Db;
use crate::dedup::DedupMap;
use crate::embed::Embedder;
use crate::ollama::OllamaClient;

/// Hifz library facade — same shape as the former `web::AppState`, but exposed
/// as a public type so any Rust caller can embed Hifz in-process without going
/// through the REST server.
///
/// The `Clone` impl is cheap: every shared field is `Arc<...>`, and `Surreal<Db>`
/// is internally Arc'd. Library users can clone freely or wrap in `Arc<Hifz>`.
#[derive(Clone)]
pub struct Hifz {
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

impl Hifz {
    /// Open a persistent SurrealKV-backed Hifz at `db_path`.
    /// Connects to the DB, runs the schema migration, loads fastembed,
    /// optionally connects Ollama if `OLLAMA_URL` env var is set,
    /// and resolves the `git` binary on `PATH` for commit enrichment.
    pub async fn open_persistent(db_path: &str) -> Result<Self> {
        let db = crate::db::connect(db_path).await?;
        Self::finish_open(db).await
    }

    /// Open an in-memory ephemeral Hifz. Otherwise identical to `open_persistent`.
    pub async fn open_memory() -> Result<Self> {
        let db = crate::db::connect_mem().await?;
        Self::finish_open(db).await
    }

    /// Construct from pre-built components — for tests, custom embedders,
    /// or when the caller wants to control config explicitly.
    pub fn from_components(
        db: Surreal<Db>,
        embedder: Arc<Embedder>,
        ollama: Option<Arc<OllamaClient>>,
        auto_compress: bool,
        token_budget: usize,
        llm_evolve: bool,
        git_path: Option<PathBuf>,
    ) -> Self {
        Self {
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
        }
    }

    /// Whether `project` is a git working tree. Cached per-project for the
    /// lifetime of the `Hifz` instance.
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

    // -----------------------------------------------------------------
    // Library API methods — one per REST handler.
    // Each is a thin dispatch into a core module function.
    // -----------------------------------------------------------------

    // --- Sessions ---
    pub async fn session_start(
        &self,
        req: crate::models::SessionStartReq,
    ) -> Result<serde_json::Value> {
        crate::session::start(&self.db, &self.embedder, self.token_budget, req).await
    }
    pub async fn session_end(&self, session_id: &str) -> Result<serde_json::Value> {
        crate::session::end(&self.db, session_id).await
    }
    pub async fn session_get(&self, id: &str) -> Result<serde_json::Value> {
        crate::session::get(&self.db, id).await
    }
    pub async fn sessions_list(&self, limit: usize) -> Result<serde_json::Value> {
        crate::session::list(&self.db, limit).await
    }

    // --- Observations ---
    pub async fn observe(&self, payload: crate::models::HookPayload) -> Result<Option<String>> {
        crate::observe::observe(
            &self.db,
            &self.dedup,
            &self.embedder,
            self.ollama.as_deref(),
            self.auto_compress,
            payload,
        )
        .await
    }
    pub async fn observations_search(
        &self,
        params: crate::models::ObservationsReq,
    ) -> Result<serde_json::Value> {
        crate::observe::search(&self.db, params).await
    }

    // --- Events (raw ledger) ---
    pub async fn event_ingest(&self, ev: crate::models::EventRequest) -> Result<serde_json::Value> {
        crate::event::ingest(&self.db, ev).await
    }
    pub async fn event_ingest_batch(
        &self,
        evs: Vec<crate::models::EventRequest>,
    ) -> Result<serde_json::Value> {
        crate::event::ingest_batch(&self.db, evs).await
    }
    pub async fn events_list(
        &self,
        params: crate::models::EventsListReq,
    ) -> Result<serde_json::Value> {
        crate::event::list(&self.db, params).await
    }
    pub async fn event_get(&self, id: &str) -> Result<serde_json::Value> {
        crate::event::get(&self.db, id).await
    }

    // --- Runs ---
    pub async fn runs_search(&self, req: crate::models::RunsReq) -> Result<serde_json::Value> {
        let limit = req.limit.unwrap_or(10);
        let rows = crate::run::search(&self.db, req.project.as_deref(), &req.query, limit).await?;
        let count = rows.len();
        Ok(serde_json::json!({"runs": rows, "count": count}))
    }
    pub async fn run_detail(&self, id: &str) -> Result<serde_json::Value> {
        crate::run::detail(&self.db, id).await
    }

    // --- Memory ---
    pub async fn remember(&self, req: crate::models::RememberReq) -> Result<serde_json::Value> {
        let category = req.category.as_deref().unwrap_or("fact");
        let keywords = req.keywords.unwrap_or_default();
        let files = req.files.unwrap_or_default();
        let project = req.project.as_deref().unwrap_or("global");
        let title = crate::remember::save(
            &self.db,
            &self.embedder,
            project,
            category,
            &req.title,
            &req.content,
            &keywords,
            &files,
            req.session_id.as_deref(),
        )
        .await?;
        Ok(serde_json::json!({"status": "ok", "title": title}))
    }
    pub async fn memories_search(
        &self,
        params: crate::models::MemoriesReq,
    ) -> Result<serde_json::Value> {
        crate::remember::search(&self.db, params).await
    }
    pub async fn forget(&self, id: &str) -> Result<()> {
        crate::remember::forget(&self.db, id).await
    }
    pub async fn evolve(&self, id: &str) -> Result<serde_json::Value> {
        let Some(ollama) = self.ollama.as_deref() else {
            return Err(anyhow::anyhow!(
                "Memory evolution requires OLLAMA_URL to be configured"
            ));
        };
        let memory_id = if id.starts_with("memory:") {
            id.to_string()
        } else {
            format!("memory:{id}")
        };
        let mut resp = self
            .db
            .query("SELECT id FROM type::record($id)")
            .bind(("id", memory_id))
            .await?;
        #[derive(Debug, surrealdb::types::SurrealValue)]
        struct Row {
            id: Option<surrealdb::types::RecordId>,
        }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        let Some(rid) = rows.into_iter().next().and_then(|r| r.id) else {
            return Err(anyhow::anyhow!("memory not found"));
        };
        let report = crate::evolve::evolve_one(&self.db, ollama, &rid).await?;
        Ok(serde_json::to_value(report).unwrap_or_default())
    }
    pub async fn memory_links(&self, id: &str) -> Result<serde_json::Value> {
        crate::link::list_for(&self.db, id).await
    }

    // --- Search & context ---
    pub async fn smart_search(&self, req: crate::models::SearchReq) -> Result<serde_json::Value> {
        let limit = req.limit.unwrap_or(10);
        let mode = req.mode.as_deref().unwrap_or("hybrid");
        let project = req.project.as_deref();
        let results = match mode {
            "text" => crate::search::search_text(&self.db, &req.query, limit, project).await,
            "semantic" => {
                crate::search::search_semantic(&self.db, &self.embedder, &req.query, limit).await
            }
            _ => {
                let cfg = crate::search::SearchConfig {
                    skip_graph: true,
                    ..Default::default()
                };
                crate::search::search_hybrid_with_config(
                    &self.db,
                    &self.embedder,
                    &req.query,
                    limit,
                    project,
                    cfg,
                )
                .await
            }
        }?;
        let count = results.len();
        Ok(serde_json::json!({"results": results, "count": count}))
    }
    pub async fn search_agentic(&self, req: crate::models::SearchReq) -> Result<serde_json::Value> {
        let limit = req.limit.unwrap_or(10);
        let project = req.project.as_deref();
        let results =
            crate::search::search_hybrid(&self.db, &self.embedder, &req.query, limit, project)
                .await?;
        let count = results.len();
        Ok(serde_json::json!({"results": results, "count": count}))
    }
    pub async fn context(&self, req: crate::models::ContextReq) -> Result<String> {
        let token_budget = req.token_budget.unwrap_or(self.token_budget);
        let context = crate::context::generate_context_with_query(
            &self.db,
            Some(&self.embedder),
            &req.project,
            req.query.as_deref(),
            token_budget,
        )
        .await
        .unwrap_or_default();
        Ok(context)
    }
    pub async fn trace(&self, req: crate::models::TraceReq) -> Result<serde_json::Value> {
        let direction = match req.direction.as_deref() {
            Some("incoming") | Some("in") => crate::link::Direction::Incoming,
            Some("both") => crate::link::Direction::Both,
            _ => crate::link::Direction::Outgoing,
        };
        let max_hops = req.max_hops.unwrap_or(2);
        let cfg = crate::link::GraphExpandConfig {
            max_hops,
            relations: req.relations.clone(),
            direction,
            ..Default::default()
        };
        let (table, key) = if let Some(idx) = req.id.find(':') {
            (req.id[..idx].to_string(), req.id[idx + 1..].to_string())
        } else {
            ("memory".to_string(), req.id.clone())
        };
        let start_rid = surrealdb::types::RecordId::new(table, key);
        let edges = crate::link::expand_graph(&self.db, &[start_rid], &cfg).await?;
        Ok(
            serde_json::json!({"edges": edges.iter().map(|e| serde_json::json!({
            "from": format!("{:?}", e.from),
            "to": format!("{:?}", e.to),
            "relation": e.relation,
            "via": e.via,
            "score": e.score,
        })).collect::<Vec<_>>(), "count": edges.len()}),
        )
    }

    // --- Core memory ---
    pub async fn core_get(&self, project: &str) -> Result<serde_json::Value> {
        let row = crate::core_mem::get(&self.db, project).await?;
        Ok(serde_json::to_value(row).unwrap_or_default())
    }
    pub async fn core_edit(
        &self,
        project: &str,
        edit: crate::models::CoreEditReq,
    ) -> Result<serde_json::Value> {
        let row =
            crate::core_mem::edit(&self.db, project, &edit.field, &edit.op, &edit.value).await?;
        Ok(serde_json::to_value(row).unwrap_or_default())
    }

    // --- Project intelligence ---
    pub async fn digest(&self, project: Option<&str>) -> Result<serde_json::Value> {
        let p = project.unwrap_or("global");
        let d = crate::digest::generate_digest(&self.db, p).await?;
        Ok(serde_json::to_value(d).unwrap_or_default())
    }
    pub async fn timeline(&self, params: crate::models::TimelineReq) -> Result<serde_json::Value> {
        crate::timeline::list(&self.db, params).await
    }
    pub async fn commits_list(
        &self,
        params: crate::models::CommitsReq,
    ) -> Result<serde_json::Value> {
        crate::commits::list(&self.db, params).await
    }
    pub async fn commit_diff(&self, sha: &str) -> Result<serde_json::Value> {
        crate::commits::diff(&self.db, self.git_path.as_ref(), sha).await
    }

    // --- Plans ---
    pub async fn plans_list(
        &self,
        params: crate::models::PlansListReq,
    ) -> Result<serde_json::Value> {
        crate::plans::list(&self.db, params).await
    }
    pub async fn plan_current(&self, project: Option<&str>) -> Result<serde_json::Value> {
        crate::plans::current(&self.db, project).await
    }
    pub async fn plan_activate(
        &self,
        req: crate::models::PlanActivateReq,
    ) -> Result<serde_json::Value> {
        crate::plans::activate(&self.db, req).await
    }
    pub async fn plan_complete(&self, id: &str) -> Result<serde_json::Value> {
        crate::plans::complete(&self.db, id).await
    }
    pub async fn plan_abandon(&self, id: &str) -> Result<serde_json::Value> {
        crate::plans::abandon(&self.db, id).await
    }

    // --- Maintenance ---
    pub async fn consolidate(&self) -> Result<serde_json::Value> {
        let r = crate::consolidate::run_consolidation(&self.db, self.ollama.as_deref()).await?;
        Ok(serde_json::to_value(r).unwrap_or_default())
    }
    pub async fn forget_gc(&self, dry_run: bool) -> Result<serde_json::Value> {
        let r = crate::forget::run_forget(&self.db, dry_run).await?;
        Ok(serde_json::to_value(r).unwrap_or_default())
    }
    pub async fn export(&self, params: crate::models::ExportReq) -> Result<serde_json::Value> {
        crate::export::run(&self.db, params).await
    }
    pub async fn health(&self) -> Result<serde_json::Value> {
        crate::health::report(
            &self.db,
            &self.embedder,
            self.started_at,
            self.ollama.is_some(),
            self.git_path.as_ref(),
        )
        .await
    }

    /// Shared finalisation for `open_*`: schema migration, fastembed,
    /// optional Ollama, git path, config.
    async fn finish_open(db: Surreal<Db>) -> Result<Self> {
        let embedder = Arc::new(Embedder::new()?);
        crate::db::init_schema(&db, embedder.dimension()).await?;

        let cfg = crate::config::load_config();

        let ollama = cfg.ollama_url.as_ref().map(|url| {
            Arc::new(OllamaClient::new(
                Some(url.clone()),
                Some(cfg.ollama_model.clone()),
            ))
        });

        let git_path = which::which("git").ok();

        Ok(Self {
            db,
            embedder,
            ollama,
            dedup: Arc::new(DedupMap::new()),
            auto_compress: cfg.auto_compress,
            token_budget: cfg.token_budget,
            llm_evolve: cfg.llm_evolve,
            started_at: Instant::now(),
            git_path,
            git_repo_cache: Arc::new(DashMap::new()),
        })
    }
}

/// Truncate a string at the largest char boundary `<= max_bytes`.
///
/// Plain `&s[..max_bytes]` panics when `max_bytes` lands inside a multi-byte
/// UTF-8 codepoint (e.g. shell-prompt glyphs `✗`, `➜`, box-drawing `─`).
/// Use this anywhere you'd otherwise byte-slice user-supplied text.
pub fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
