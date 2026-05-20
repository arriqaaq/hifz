use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use dashmap::DashMap;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

pub mod chunk;
pub mod code;
pub mod commits;
pub mod compress;
pub use kernel::config;
pub mod consolidate;
pub mod context;
pub mod core_mem;
pub use kernel::db;
pub mod dedup;
pub mod delta;
pub mod digest;
pub use kernel::embed;
pub mod enrich;
pub mod error;
pub mod evolve;
pub mod export;
pub mod forget;
pub mod githook;
pub mod ground;
pub mod health;
pub mod link;
pub mod llm_rerank;
pub mod markdown;
pub mod mcp;
pub use kernel::models;
pub mod observe;
pub use kernel::ollama;
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
pub mod usage;
pub mod warmup;
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

    // Library API methods — one per REST handler.
    // Each is a thin dispatch into a core module function.

    // Sessions
    pub async fn session_start(
        &self,
        req: crate::models::SessionStartReq,
    ) -> Result<serde_json::Value> {
        crate::session::start(&self.db, &self.embedder, self.token_budget, req).await
    }
    pub async fn session_end(&self, session_id: &str) -> Result<serde_json::Value> {
        let result = crate::session::end(&self.db, session_id).await?;

        // Kick off consolidation in the background. Time-bounded
        // by the consolidator itself; spawned so the SessionEnd hook returns
        // immediately and the agent's shutdown isn't blocked by LLM calls.
        if let Some(ollama) = self.ollama.clone() {
            let db = self.db.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    crate::consolidate::run_consolidation(&db, Some(ollama.as_ref())).await
                {
                    tracing::warn!("session-end consolidation failed: {e}");
                }
            });
        } else {
            tracing::debug!(
                "session-end consolidation skipped: Ollama not configured (deterministic decay still ran)"
            );
        }

        Ok(result)
    }

    /// Build a project-scoped warmup digest at session start.
    /// Returns the typed `WarmupDigest` (latest_plan, decisions, conventions,
    /// open_bugs, gotchas, failure_patterns, recent_lessons, plus a flat top-N).
    /// Hook callers should inject `top` as a system-context block.
    pub async fn session_warmup(
        &self,
        session_id: &str,
        project: Option<&str>,
        top_n: Option<usize>,
    ) -> Result<serde_json::Value> {
        // Resolve project from explicit param OR by looking up the session row.
        let project_resolved = match project {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => {
                #[derive(Debug, surrealdb::types::SurrealValue)]
                struct Row {
                    project: Option<String>,
                }
                let sid = if session_id.starts_with("session:") {
                    session_id.to_string()
                } else {
                    format!("session:{session_id}")
                };
                let mut resp = self
                    .db
                    .query("SELECT project FROM type::record($id)")
                    .bind(("id", sid))
                    .await?;
                let rows: Vec<Row> = resp.take(0).unwrap_or_default();
                rows.into_iter()
                    .next()
                    .and_then(|r| r.project)
                    .unwrap_or_else(|| "global".to_string())
            }
        };

        let digest =
            crate::warmup::build_warmup(&self.db, &project_resolved, Some(session_id), top_n)
                .await?;
        Ok(serde_json::to_value(digest).unwrap_or_default())
    }

    pub async fn session_get(&self, id: &str) -> Result<serde_json::Value> {
        crate::session::get(&self.db, id).await
    }
    pub async fn sessions_list(&self, limit: usize) -> Result<serde_json::Value> {
        crate::session::list(&self.db, limit).await
    }

    // Observations
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

    // Runs
    pub async fn runs_search(&self, req: crate::models::RunsReq) -> Result<serde_json::Value> {
        let limit = req.limit.unwrap_or(10);
        let rows = crate::run::search(&self.db, req.project.as_deref(), &req.query, limit).await?;
        let count = rows.len();
        Ok(serde_json::json!({"runs": rows, "count": count}))
    }
    pub async fn run_detail(&self, id: &str) -> Result<serde_json::Value> {
        crate::run::detail(&self.db, id).await
    }
    /// Tree view of one session: session + its runs + observations.
    /// Reuses `session::get`, `run::list_by_session`, and `observe::search`
    /// so this helper stays consistent with every other read path.
    pub async fn session_tree(&self, id: &str) -> Result<serde_json::Value> {
        let session_value = crate::session::get(&self.db, id).await?;
        // session::get returns `{"error": ...}` on miss; normalize to null.
        let session = match session_value.get("error") {
            Some(_) => serde_json::Value::Null,
            None => session_value,
        };

        let runs = crate::run::list_by_session(&self.db, id).await?;

        let obs_resp = crate::observe::search(
            &self.db,
            crate::models::ObservationsReq {
                session_id: Some(id.trim_start_matches("session:").to_string()),
                limit: Some(500),
                ..Default::default()
            },
        )
        .await?;
        // observe::search returns DESC; flip to ASC for the trace tree.
        let mut observations: Vec<serde_json::Value> = obs_resp
            .get("observations")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        observations.sort_by(|a, b| {
            let ta = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            let tb = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            ta.cmp(tb)
        });

        Ok(serde_json::json!({
            "session": session,
            "runs": runs,
            "observations": observations,
        }))
    }

    // Memory
    pub async fn remember(&self, req: crate::models::RememberReq) -> Result<serde_json::Value> {
        // Default category is `Note` (catch-all for ad-hoc memories) — see
        // `models::Category`. Unknown category strings also fall through to Note.
        let category = req
            .category
            .as_deref()
            .map(crate::models::Category::from_str)
            .unwrap_or(crate::models::Category::Note);
        let keywords = req.keywords.unwrap_or_default();
        let files = req.files.unwrap_or_default();
        let tags = req.tags.unwrap_or_default();
        let project = req.project.as_deref().unwrap_or("global");

        // `&'static str`, so it survives `category` being moved into the call.
        let category_label = category.as_str();
        // `title` is optional/descriptive (memories are keyed by RecordId).
        // Use the caller's value only when present and non-blank; otherwise
        // derive a non-empty headline from `content`. This is the single
        // chokepoint for both the REST endpoint and the MCP proxy, so the
        // "LLM forgot title" failure cannot reach storage.
        let title = req
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| crate::enrich::derive_title(&req.content, category_label));
        let id = crate::enrich::save_enriched(
            &self.db,
            &self.embedder,
            self.ollama.as_deref(),
            self.llm_evolve,
            project,
            &title,
            &req.content,
            category,
            keywords,
            files,
            tags,
            req.content_long.clone(),
            req.closes_memory_id.as_deref(),
            req.supersedes_memory_id.as_deref(),
            req.session_id.as_deref(),
        )
        .await?;
        let changes = crate::delta::save_changes(
            &id,
            &title,
            category_label,
            req.supersedes_memory_id.as_deref(),
            req.closes_memory_id.as_deref(),
        );
        // Render once; record to the session timeline (replayable) and
        // attach the same delta to the response.
        let delta = memdiff::delta_from_changes(&changes);
        crate::delta::record_observation(&self.db, req.session_id.as_deref(), &delta).await;
        Ok(crate::delta::attach_delta(
            serde_json::json!({"status": "ok", "id": id, "title": title}),
            delta,
        ))
    }
    pub async fn memories_search(
        &self,
        params: crate::models::MemoriesReq,
    ) -> Result<serde_json::Value> {
        crate::remember::search(&self.db, params).await
    }
    pub async fn forget(&self, id: &str) -> Result<serde_json::Value> {
        crate::remember::forget(&self.db, id).await?;
        let changes = [memdiff::Change::Forgotten { id: id.to_string() }];
        Ok(crate::delta::attach(
            serde_json::json!({"status": "ok"}),
            &changes,
        ))
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
            return Err(crate::error::HifzError::NotFound("memory not found".into()).into());
        };
        let outcome = crate::evolve::evolve_one(&self.db, ollama, &rid).await?;
        Ok(crate::delta::attach(
            serde_json::json!({ "report": outcome.report }),
            &outcome.changes,
        ))
    }
    pub async fn memory_links(&self, id: &str) -> Result<serde_json::Value> {
        crate::link::list_for(&self.db, id).await
    }
    /// Inspect view: a memory plus its lineage (superseded rows), outgoing
    /// links, and evolution history, rendered as a structured `MemoryView`.
    pub async fn memory_view(&self, id: &str) -> Result<serde_json::Value> {
        let mid = if id.starts_with("memory:") {
            id.to_string()
        } else {
            format!("memory:{id}")
        };
        #[derive(Debug, surrealdb::types::SurrealValue)]
        struct Row {
            title: Option<String>,
            category: Option<String>,
            supersedes: Option<Vec<surrealdb::types::RecordId>>,
            evolution_history: Option<Vec<crate::models::EvolutionEntry>>,
        }
        let mut resp = self
            .db
            .query(
                "SELECT title, category, supersedes, evolution_history \
                 FROM type::record($id)",
            )
            .bind(("id", mid.clone()))
            .await?;
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            return Err(crate::error::HifzError::NotFound("memory not found".into()).into());
        };
        let title = row.title.unwrap_or_default();
        let category = row.category.unwrap_or_else(|| "note".into());
        let superseded: Vec<String> = row
            .supersedes
            .unwrap_or_default()
            .iter()
            .map(crate::rid_to_string)
            .collect();
        let links = crate::link::list_for(&self.db, &mid).await?;
        let history = row.evolution_history.unwrap_or_default();
        let changes = crate::delta::view_changes(&mid, &superseded, &links, &history);
        let view = memdiff::view_of(&title, &category, &mid, &changes);
        Ok(memdiff::sink_json::view_to_value(&view))
    }

    /// Sessions that have recorded memory-delta events (the existing
    /// `observation` timeline, `obs_type='memory_delta'`).
    pub async fn replays_list(&self) -> Result<serde_json::Value> {
        let mut resp = self
            .db
            .query(
                "SELECT meta::id(session_id) AS session_id, count() AS count, \
                 math::max(timestamp) AS last_ts \
                 FROM observation WHERE obs_type = 'memory_delta' GROUP BY session_id",
            )
            .await?;
        let mut replays: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        // Most recent activity first.
        replays.sort_by(|a, b| {
            b.get("last_ts")
                .and_then(|v| v.as_str())
                .cmp(&a.get("last_ts").and_then(|v| v.as_str()))
        });
        let count = replays.len();
        Ok(serde_json::json!({"replays": replays, "count": count}))
    }

    /// Ordered memory-delta events for one session, as `SessionEvent::Delta`
    /// JSON (`{kind:"delta", t, delta}`) — the replay player's input.
    pub async fn replay_get(&self, id: &str) -> Result<serde_json::Value> {
        if id.is_empty() {
            return Err(crate::error::HifzError::InvalidInput("empty session id".into()).into());
        }
        let sid = format!("session:{}", id.strip_prefix("session:").unwrap_or(id));
        let mut resp = self
            .db
            .query(
                "SELECT timestamp, ord, metadata FROM observation \
                 WHERE obs_type = 'memory_delta' AND session_id = type::record($sid) \
                 ORDER BY ord ASC",
            )
            .bind(("sid", sid))
            .await?;
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        let events: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let t = r
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let delta = r
                    .get("metadata")
                    .and_then(|m| m.get("delta"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({ "lines": [] }));
                serde_json::json!({ "kind": "delta", "t": t, "delta": delta })
            })
            .collect();
        let count = events.len();
        Ok(serde_json::json!({"session_id": id, "events": events, "count": count}))
    }

    /// Typed graph walk from a memory. `relations` filters which
    /// edge labels are traversed; `max_hops` defaults to 1 (immediate
    /// neighbors). Returns the neighbor memory rows annotated with the
    /// edge that pulled them in (relation, score, via, reason).
    pub async fn memory_neighbors(
        &self,
        id: &str,
        relations: Option<Vec<String>>,
        max_hops: Option<usize>,
    ) -> Result<serde_json::Value> {
        let normalized = if id.starts_with("memory:") {
            id.to_string()
        } else {
            format!("memory:{id}")
        };
        #[derive(Debug, surrealdb::types::SurrealValue)]
        struct Row {
            id: Option<surrealdb::types::RecordId>,
        }
        let mut resp = self
            .db
            .query("SELECT id FROM type::record($id)")
            .bind(("id", normalized))
            .await?;
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        let Some(rid) = rows.into_iter().next().and_then(|r| r.id) else {
            return Err(
                crate::error::HifzError::NotFound(format!("memory not found: {id}")).into(),
            );
        };

        let cfg = crate::link::GraphExpandConfig {
            max_hops: max_hops.unwrap_or(1).clamp(1, 4),
            relations,
            min_score: 0.0,
            dampening: 0.5,
            max_results: 50,
            direction: crate::link::Direction::Outgoing,
        };
        let edges = crate::link::expand_graph(&self.db, &[rid], &cfg).await?;

        // Hydrate the neighbor memory rows for nicer presentation.
        let neighbor_ids: Vec<surrealdb::types::RecordId> =
            edges.iter().map(|e| e.to.clone()).collect();
        let mut neighbor_titles: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        if !neighbor_ids.is_empty() {
            #[derive(Debug, surrealdb::types::SurrealValue)]
            struct N {
                id: Option<surrealdb::types::RecordId>,
                title: Option<String>,
                category: Option<String>,
                context_summary: Option<String>,
            }
            let mut resp = self
                .db
                .query("SELECT id, title, category, context_summary FROM memory WHERE id IN $ids")
                .bind(("ids", neighbor_ids))
                .await?;
            let ns: Vec<N> = resp.take(0).unwrap_or_default();
            for n in ns {
                if let Some(rid) = n.id {
                    neighbor_titles.insert(
                        format!("{rid:?}"),
                        serde_json::json!({
                            "title": n.title,
                            "category": n.category,
                            "context_summary": n.context_summary,
                        }),
                    );
                }
            }
        }

        let neighbors: Vec<serde_json::Value> = edges
            .into_iter()
            .map(|e| {
                let to_str = format!("{:?}", e.to);
                let meta = neighbor_titles.get(&to_str).cloned().unwrap_or_default();
                serde_json::json!({
                    "id": to_str,
                    "relation": e.relation,
                    "via": e.via,
                    "score": e.score,
                    "reason": e.reason,
                    "title": meta.get("title"),
                    "category": meta.get("category"),
                    "context_summary": meta.get("context_summary"),
                })
            })
            .collect();
        let count = neighbors.len();
        Ok(serde_json::json!({"neighbors": neighbors, "count": count}))
    }

    /// Chronological digest of recent memories for a project.
    /// Groups the last `days` (default 30) of activity by category.
    pub async fn project_digest(
        &self,
        project: &str,
        days: Option<i64>,
    ) -> Result<serde_json::Value> {
        let days = days.unwrap_or(30).max(1);
        let since = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();

        #[derive(Debug, surrealdb::types::SurrealValue)]
        struct Row {
            id: Option<surrealdb::types::RecordId>,
            category: Option<String>,
            title: Option<String>,
            context_summary: Option<String>,
            content: Option<String>,
            created_at: Option<String>,
        }
        let mut resp = self
            .db
            .query(
                "SELECT id, category, title, context_summary, content, created_at \
                 FROM memory \
                 WHERE is_latest = true \
                   AND (project = $project OR project = 'global') \
                   AND created_at >= $since \
                 ORDER BY created_at DESC \
                 LIMIT 100",
            )
            .bind(("project", project.to_string()))
            .bind(("since", since.clone()))
            .await?;
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();

        // Group by category preserving the chronological order within each bucket.
        let mut by_cat: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for r in rows {
            let cat = r.category.unwrap_or_else(|| "note".to_string());
            let summary = r.context_summary.unwrap_or_else(|| {
                let c = r.content.unwrap_or_default();
                if c.len() > 200 {
                    format!("{}…", &c[..200])
                } else {
                    c
                }
            });
            by_cat.entry(cat).or_default().push(serde_json::json!({
                "id": r.id.map(|x| format!("{x:?}")),
                "title": r.title,
                "summary": summary,
                "created_at": r.created_at,
            }));
        }
        Ok(serde_json::json!({
            "project": project,
            "days": days,
            "since": since,
            "by_category": by_cat,
        }))
    }

    /// Project accumulator rollup. Mirrors the warmup digest in
    /// shape (latest plan, decisions, conventions, open bugs, gotchas,
    /// failure patterns, recent lessons) but is project-only — no
    /// session_id concept and no flat top-N.
    pub async fn project_accumulators(&self, project: &str) -> Result<serde_json::Value> {
        let digest = crate::warmup::build_warmup(&self.db, project, None, Some(50)).await?;
        Ok(serde_json::to_value(digest).unwrap_or_default())
    }

    /// List incoming edges for a memory ("backlinks" — every
    /// memory or observation that references this one). Optional relation
    /// filter narrows to a single typed edge.
    pub async fn memory_backlinks(
        &self,
        id: &str,
        relation: Option<&str>,
    ) -> Result<serde_json::Value> {
        let normalized = if id.starts_with("memory:") {
            id.to_string()
        } else {
            format!("memory:{id}")
        };

        let mut sql = String::from(
            "SELECT in.id AS id, in.title AS title, in.category AS category, \
             relation, score, via, reason FROM edge \
             WHERE out = type::record($id)",
        );
        if relation.is_some() {
            sql.push_str(" AND relation = $rel");
        }
        let mut q = self.db.query(&sql).bind(("id", normalized));
        if let Some(r) = relation {
            q = q.bind(("rel", r.to_string()));
        }
        let mut resp = q.await?;
        let backlinks: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        let count = backlinks.len();
        Ok(serde_json::json!({"backlinks": backlinks, "count": count}))
    }

    /// Render a memory as a frontmatter-rich markdown document.
    pub async fn memory_markdown_get(&self, id: &str) -> Result<String> {
        crate::markdown::render(&self.db, id).await
    }

    /// Parse an edited markdown document and write it as a NEW
    /// memory version that supersedes the old one. Old memory's
    /// `is_latest` is flipped via the supersedes lifecycle path in
    /// `enrich::save_enriched`.
    pub async fn memory_markdown_put(&self, id: &str, body: &str) -> Result<serde_json::Value> {
        let doc = crate::markdown::parse(body)?;
        let project = doc
            .frontmatter
            .project
            .clone()
            .unwrap_or_else(|| "global".to_string());
        let category = doc
            .frontmatter
            .category
            .as_deref()
            .map(crate::models::Category::from_str)
            .unwrap_or(crate::models::Category::Note);
        let title = doc
            .frontmatter
            .title
            .clone()
            .unwrap_or_else(|| "Untitled".to_string());

        // The body is the full document. For long-form categories, store
        // the body as `content_long` and derive a summary `content` from
        // the first ~300 chars; for short categories, body is the content.
        let (content, content_long): (String, Option<String>) = if category.is_long_form() {
            let summary: String = doc.body.chars().take(300).collect();
            (summary, Some(doc.body.clone()))
        } else {
            (doc.body.clone(), None)
        };

        // The old chunks are deleted before the new version is written
        // so a re-PUT doesn't accumulate stale chunks.
        if let Some(old_rid) = resolve_memory_record_id(&self.db, id).await {
            let _ = crate::chunk::delete_chunks_for(&self.db, &old_rid).await;
        }

        let new_id = crate::enrich::save_enriched(
            &self.db,
            &self.embedder,
            self.ollama.as_deref(),
            self.llm_evolve,
            &project,
            &title,
            &content,
            category,
            doc.frontmatter.keywords.clone(),
            doc.frontmatter.files.clone(),
            doc.frontmatter.tags.clone(),
            content_long,
            None,
            Some(id),
            None,
        )
        .await?;

        Ok(serde_json::json!({"status": "ok", "id": new_id, "supersedes": id}))
    }

    // Search & context
    pub async fn search(&self, req: crate::models::SearchReq) -> Result<serde_json::Value> {
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
    pub async fn search_session(&self, req: crate::models::SearchReq) -> Result<serde_json::Value> {
        let limit = req.limit.unwrap_or(10);
        let project = req.project.as_deref();
        let results =
            crate::search::search_hybrid(&self.db, &self.embedder, &req.query, limit, project)
                .await?;
        let count = results.len();
        Ok(serde_json::json!({"results": results, "count": count}))
    }

    // Code dimension

    pub async fn code_index(&self, req: crate::models::CodeIndexReq) -> Result<serde_json::Value> {
        let opts = crate::code::index::IndexOpts {
            follow_symlinks: req.follow_symlinks.unwrap_or(false),
            max_file_bytes: req.max_file_bytes.unwrap_or(2 * 1024 * 1024),
            ..Default::default()
        };
        let report = crate::code::index::index_repo(
            &self.db,
            &self.embedder,
            &req.project,
            std::path::Path::new(&req.root),
            &opts,
        )
        .await?;
        Ok(serde_json::to_value(&report)?)
    }

    pub async fn code_search(
        &self,
        req: crate::models::CodeSearchReq,
    ) -> Result<serde_json::Value> {
        let opts = crate::code::search::CodeSearchOpts {
            limit: req.limit.unwrap_or(10),
            project: req.project,
            language: req.language,
            path: req.path,
            group_by_file: req.group_by_file.unwrap_or(false),
        };
        let results =
            crate::code::search::search_code(&self.db, &self.embedder, &req.query, &opts).await?;
        let count = results.len();
        Ok(serde_json::json!({ "results": results, "count": count }))
    }

    pub async fn code_link(&self, req: crate::models::CodeLinkReq) -> Result<serde_json::Value> {
        let mid = if req.memory_id.starts_with("memory:") {
            req.memory_id.clone()
        } else {
            format!("memory:{}", req.memory_id)
        };
        let memory_id = surrealdb::types::RecordId::new(
            "memory".to_string(),
            mid.trim_start_matches("memory:").to_string(),
        );
        let project = req.project.as_deref().unwrap_or("global");
        let linked = crate::code::link::link_memory_to_lines(
            &self.db,
            &memory_id,
            project,
            &req.file,
            req.start_line,
            req.end_line,
            req.reason.as_deref(),
        )
        .await?;
        Ok(serde_json::json!({
            "memory_id": mid,
            "linked_chunks": linked
                .iter()
                .map(|r| format!("{r:?}"))
                .collect::<Vec<_>>(),
            "via": "reference",
        }))
    }

    pub async fn code_link_symbol(
        &self,
        req: crate::models::CodeLinkSymReq,
    ) -> Result<serde_json::Value> {
        let mid = if req.memory_id.starts_with("memory:") {
            req.memory_id.clone()
        } else {
            format!("memory:{}", req.memory_id)
        };
        let memory_id = surrealdb::types::RecordId::new(
            "memory".to_string(),
            mid.trim_start_matches("memory:").to_string(),
        );
        let project = req.project.as_deref().unwrap_or("global");
        let linked = crate::code::link::link_memory_to_symbol(
            &self.db,
            &memory_id,
            project,
            &req.name,
            req.kind.as_deref(),
            req.file.as_deref(),
            req.reason.as_deref(),
        )
        .await?;
        Ok(serde_json::json!({
            "memory_id": mid,
            "linked_symbols": linked
                .iter()
                .map(|r| format!("{r:?}"))
                .collect::<Vec<_>>(),
            "via": "reference",
        }))
    }

    pub async fn code_gc(&self, req: crate::models::CodeGcReq) -> Result<serde_json::Value> {
        let report = crate::code::gc::run_gc(
            &self.db,
            &req.project,
            std::path::Path::new(&req.root),
            req.dry_run.unwrap_or(false),
            req.force_decay.unwrap_or(false),
        )
        .await?;
        Ok(serde_json::to_value(&report)?)
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
        // Multi-seed when `ids` is provided (convergence/divergence);
        // else single `id`. `expand_graph` is already multi-seed.
        let id_list: Vec<String> = match &req.ids {
            Some(v) if !v.is_empty() => v.clone(),
            _ => vec![req.id.clone()],
        };
        let seeds: Vec<surrealdb::types::RecordId> = id_list
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| {
                let (table, key) = if let Some(idx) = s.find(':') {
                    (s[..idx].to_string(), s[idx + 1..].to_string())
                } else {
                    ("memory".to_string(), s.clone())
                };
                surrealdb::types::RecordId::new(table, key)
            })
            .collect();
        let edges = crate::link::expand_graph(&self.db, &seeds, &cfg).await?;
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

    // Core memory
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

    // Project intelligence
    pub async fn digest(&self, project: Option<&str>) -> Result<serde_json::Value> {
        let p = project.unwrap_or("global");
        let d = crate::digest::generate_digest(&self.db, p).await?;
        Ok(serde_json::to_value(d).unwrap_or_default())
    }
    pub async fn timeline(&self, params: crate::models::TimelineReq) -> Result<serde_json::Value> {
        crate::timeline::list(&self.db, params).await
    }
    /// Causal timeline: time-ordered provenance chain from a seed (or the
    /// project's active plan + recent commits) over causal relations only.
    pub async fn timeline_causal(
        &self,
        seed: Option<&str>,
        project: Option<&str>,
        max_hops: usize,
        limit: usize,
    ) -> Result<serde_json::Value> {
        let r = crate::trace::causal(&self.db, seed, project, max_hops, limit).await?;
        Ok(serde_json::to_value(r).unwrap_or_default())
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

    // Plans
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

    // Maintenance
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

    // Agent usage (generic LLM token tracking)
    pub async fn usage_record(
        &self,
        rec: crate::models::AgentUsageRecord,
    ) -> Result<crate::usage::IngestResult> {
        crate::usage::record(&self.db, rec).await
    }
    pub async fn usage_record_batch(
        &self,
        records: Vec<crate::models::AgentUsageRecord>,
    ) -> Result<crate::usage::IngestResult> {
        crate::usage::record_batch(&self.db, records).await
    }
    pub async fn usage_for_session(
        &self,
        session_id: &str,
    ) -> Result<crate::usage::SessionUsageView> {
        crate::usage::for_session(&self.db, session_id).await
    }
    pub async fn usage_for_project(
        &self,
        project: &str,
        filters: crate::usage::ProjectFilters,
    ) -> Result<crate::usage::ProjectUsageView> {
        crate::usage::for_project(&self.db, project, filters).await
    }
    pub async fn usage_session_totals(
        &self,
        project: Option<&str>,
    ) -> Result<Vec<crate::usage::aggregate::SessionRow>> {
        crate::usage::session_totals(&self.db, project).await
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

// `rid_to_string` moved to `kernel::ids`; re-exported at crate root
// (below) so `crate::rid_to_string` / `hifz::rid_to_string` still resolve.

/// Returns `None` if the row doesn't exist. Used by `Hifz::memory_markdown_put`
/// to clear stale chunks before writing the new version.
async fn resolve_memory_record_id(
    db: &Surreal<Db>,
    id: &str,
) -> Option<surrealdb::types::RecordId> {
    let normalized = if id.starts_with("memory:") {
        id.to_string()
    } else {
        format!("memory:{id}")
    };
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<surrealdb::types::RecordId>,
    }
    let mut resp = db
        .query("SELECT id FROM type::record($id)")
        .bind(("id", normalized))
        .await
        .ok()?;
    let rows: Vec<Row> = resp.take(0).ok()?;
    rows.into_iter().next().and_then(|r| r.id)
}

// `truncate_at_char_boundary` moved to `kernel::ids`; re-exported at
// crate root (below).
pub use kernel::ids::{rid_to_string, truncate_at_char_boundary};
