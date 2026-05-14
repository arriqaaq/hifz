use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::compress::{compress_llm, compress_synthetic};
use crate::db::Db;
use crate::dedup::DedupMap;
use crate::embed::Embedder;
use crate::ground;
use crate::link;
use crate::models::HookPayload;
use crate::ollama::OllamaClient;
use crate::run;

/// Capture a raw observation from an agent hook.
/// Deduplicates, compresses, embeds, and stores.
pub async fn observe(
    db: &Surreal<Db>,
    dedup: &DedupMap,
    embedder: &Embedder,
    ollama: Option<&OllamaClient>,
    auto_compress: bool,
    payload: HookPayload,
) -> Result<Option<String>> {
    // Run lifecycle — fire before dedup so lifecycle events aren't dropped.
    // Runs are task-scoped: UserPromptSubmit appends to open run or starts new.
    // TaskCompleted/Stop close the run.
    let event: crate::models::HifzEvent = payload.hook_type.as_str().into();
    match event {
        crate::models::HifzEvent::PromptSubmit => {
            let prompt = payload
                .data
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ensure_session(db, &payload).await?;

            // Check if run already open for this session
            if let Some(open_run) = run::find_open(db, &payload.session_id).await.ok().flatten() {
                let _ = run::append_prompt(db, &open_run, &prompt).await;
            } else {
                if let Some(session_rid) = run::resolve_session(db, &payload.session_id).await {
                    let _ = run::start(db, &session_rid, &payload.project, &prompt).await;
                }
            }
        }
        crate::models::HifzEvent::SessionStop | crate::models::HifzEvent::TaskCompleted => {
            if let Some(open) = run::find_open(db, &payload.session_id).await.ok().flatten() {
                let outcome = run::detect_uncommitted_outcome(db, &open).await;
                let _ = run::close(db, &open, &outcome, None).await;
            }
            // Decay memories from uncommitted runs in this session
            let _ = ground::decay_uncommitted(db, &payload.session_id).await;
        }
        _ => {}
    }

    // Dedup check
    let tool_name = payload
        .data
        .get("tool_name")
        .or_else(|| payload.data.get("toolName"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let tool_input_str = payload
        .data
        .get("tool_input")
        .or_else(|| payload.data.get("toolInput"))
        .map(|v| v.to_string())
        .unwrap_or_default();
    let hash = DedupMap::compute_hash(&payload.session_id, tool_name, &tool_input_str);
    if dedup.is_duplicate(&hash) {
        return Ok(None);
    }
    dedup.record(hash);

    // Ensure session exists
    ensure_session(db, &payload).await?;

    // Compress: synthetic (default) or LLM (optional)
    let compressed = if auto_compress {
        if let Some(ollama) = ollama {
            match compress_llm(&payload, ollama).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("LLM compression failed, falling back to synthetic: {e}");
                    compress_synthetic(&payload)
                }
            }
        } else {
            compress_synthetic(&payload)
        }
    } else {
        compress_synthetic(&payload)
    };

    let facts_text = if compressed.facts.is_empty() {
        None
    } else {
        Some(compressed.facts.join(" "))
    };

    // Generate embedding — include facts, keywords, files for richer vectors
    let mut embed_text = format!("{}\n{}", compressed.title, compressed.narrative);
    if let Some(ref ft) = facts_text {
        embed_text.push_str("\nfacts: ");
        embed_text.push_str(ft);
    }
    if !compressed.keywords.is_empty() {
        embed_text.push_str("\nkeywords: ");
        embed_text.push_str(&compressed.keywords.join(", "));
    }
    if !compressed.files.is_empty() {
        embed_text.push_str("\nfiles: ");
        embed_text.push_str(&compressed.files.join(", "));
    }
    let embedding = match embedder.embed_single(&embed_text) {
        Ok(vec) => Some(vec),
        Err(e) => {
            tracing::warn!("Embedding failed: {e}");
            None
        }
    };

    // Store in SurrealDB. Per-session monotonic `ord` allocated atomically
    // inside the transaction; the `obs_session_ord` UNIQUE index is the final
    // guard against any race between concurrent writers.
    let session_rid = format!("session:{}", payload.session_id);
    let source = payload.source.clone().unwrap_or_else(|| "hook".to_string());
    let parent_rid = payload.parent_obs_id.as_deref().map(|s| {
        if s.contains(':') {
            s.to_string()
        } else {
            format!("observation:{s}")
        }
    });
    let sql = "
        BEGIN TRANSACTION;
        LET $sess = type::record($session_rid);
        LET $next = count(SELECT id FROM observation WHERE session_id = $sess);
        LET $created = (CREATE observation SET
            session_id    = $sess,
            ord           = $next,
            parent_obs_id = IF $parent_rid = NONE THEN NONE ELSE type::record($parent_rid) END,
            source        = $source,
            timestamp     = $timestamp,
            obs_type      = $obs_type,
            title         = $title,
            subtitle      = $subtitle,
            facts         = $facts,
            facts_text    = $facts_text,
            narrative     = $narrative,
            keywords      = $keywords,
            files         = $files,
            importance    = $importance,
            confidence    = $confidence,
            embedding     = $embedding
          RETURN id, ord);
        RETURN $created;
        COMMIT TRANSACTION;
    ";

    #[derive(Debug, SurrealValue)]
    struct Created {
        id: Option<RecordId>,
    }

    let response = db
        .query(sql)
        .bind(("session_rid", session_rid))
        .bind(("parent_rid", parent_rid))
        .bind(("source", source))
        .bind(("timestamp", payload.timestamp.clone()))
        .bind(("obs_type", compressed.obs_type.clone()))
        .bind(("title", compressed.title.clone()))
        .bind(("subtitle", compressed.subtitle.clone()))
        .bind(("facts", compressed.facts.clone()))
        .bind(("facts_text", facts_text.clone()))
        .bind(("narrative", compressed.narrative.clone()))
        .bind(("keywords", compressed.keywords.clone()))
        .bind(("files", compressed.files.clone()))
        .bind(("importance", compressed.importance))
        .bind(("confidence", compressed.confidence))
        .bind(("embedding", embedding.clone()))
        .await?;
    let mut response = response.check()?;
    // Inside a transaction, only the explicit RETURN $created emits to the
    // client (BEGIN/LET/COMMIT are silent), so the CREATE result lands at
    // statement index 0.
    let created: Vec<Created> = response.take(0).unwrap_or_default();
    let new_obs_id = created.into_iter().next().and_then(|c| c.id);

    // Append to the open run (if any) for this session.
    if let Some(obs_id) = new_obs_id.as_ref() {
        match run::find_open(db, &payload.session_id).await {
            Ok(Some(r)) => {
                if let Err(e) = run::append(db, &r, obs_id).await {
                    tracing::warn!("run::append failed: {e}");
                }
            }
            Ok(None) => {
                tracing::debug!("no open run for session {}", payload.session_id);
            }
            Err(e) => {
                tracing::warn!("run::find_open lookup failed: {e}");
            }
        }

        // Phase 6: bridge observations into the memory graph via shared
        // file paths. Cheap deterministic edge — the agent's behavioral
        // trail (which files it touched) becomes visible to memory
        // retrieval. Skipped when the observation has no files.
        if !compressed.files.is_empty() {
            link_observation_files_to_memories(db, obs_id, &payload.project, &compressed.files)
                .await;
        }
    }

    // Increment session observation count
    db.query("UPDATE type::record($sid) SET observation_count += 1")
        .bind(("sid", format!("session:{}", payload.session_id)))
        .await?;

    // When adapter sends a commit_made observation, trigger grounding and
    // a few causal edges:
    //   - obs --generated_by--> run  (PROV-O: this commit was produced by
    //     the activity that is the run)
    //   - obs --commits_for--> memory  (Phase 3.3: when the commit message
    //     BM25-matches an open Bug/Plan/Decision, the commit closes the loop
    //     for that memory)
    if compressed.obs_type == "commit_made" {
        let _ = ground::on_commit_observation(db, &payload.project, &compressed.files).await;
        if let Some(obs_id) = new_obs_id.as_ref() {
            if let Ok(Some(run_id)) = run::find_open(db, &payload.session_id).await {
                let _ = link::upsert_edge(
                    db,
                    obs_id,
                    &run_id,
                    "generated_by",
                    "system",
                    1.0,
                    Some("commit observation produced during this run"),
                )
                .await;
            }
            let commit_msg = compressed
                .narrative
                .lines()
                .next()
                .unwrap_or(&compressed.title)
                .to_string();
            link_commit_to_open_memories(db, obs_id, &payload.project, &commit_msg).await;
        }
    }

    // Return the new observation id (e.g. "observation:abc") so hook scripts
    // can cache it and stamp `parent_obs_id` on the next correlated event
    // (e.g. SubagentStop pointing back at SubagentStart). Falls back to the
    // observation title if for any reason no id was captured.
    let returned = new_obs_id
        .as_ref()
        .map(rid_to_string)
        .unwrap_or(compressed.title);
    Ok(Some(returned))
}

/// Format a SurrealDB `RecordId` as the canonical `"table:key"` string,
/// matching how hook scripts and the rest of the codebase quote them.
fn rid_to_string(rid: &RecordId) -> String {
    use surrealdb::types::RecordIdKey;
    let key = match &rid.key {
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::Uuid(u) => u.to_string(),
        other => format!("{other:?}"),
    };
    format!("{}:{key}", rid.table)
}

/// Phase 3.3: when a `commit_made` observation arrives, BM25-search open
/// Bug/Plan/Decision memories for the commit message and write a
/// `commits_for` edge from the observation to each match. Capped at the
/// top-3 matches so a generic commit message ("fix bug") doesn't link to
/// every open bug. The score is the BM25 sum (higher = better).
async fn link_commit_to_open_memories(
    db: &Surreal<Db>,
    obs_id: &RecordId,
    project: &str,
    commit_msg: &str,
) {
    if commit_msg.trim().is_empty() {
        return;
    }

    #[derive(Debug, SurrealValue)]
    struct Hit {
        id: Option<RecordId>,
        title: Option<String>,
        category: Option<String>,
        #[surreal(rename = "_score")]
        score: Option<f64>,
    }

    // Open Bug/Plan/Decision = is_latest && no incoming `closes` edge.
    // Score by BM25 over title + content; require a positive score so empty
    // matches drop out.
    let sql = "SELECT id, title, category, \
                      search::score(1) + search::score(2) AS _score \
               FROM memory \
               WHERE is_latest = true \
                 AND category IN ['bug', 'plan', 'decision'] \
                 AND (project = $project OR project = 'global') \
                 AND id NOT IN (SELECT VALUE out FROM edge WHERE relation = 'closes') \
                 AND (title @1@ $q OR content @2@ $q) \
               ORDER BY _score DESC \
               LIMIT 3";

    let resp = db
        .query(sql)
        .bind(("project", project.to_string()))
        .bind(("q", commit_msg.to_string()))
        .await;
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("commits_for search failed: {e}");
            return;
        }
    };
    let hits: Vec<Hit> = resp.take(0).unwrap_or_default();

    for h in hits {
        let Some(mid) = h.id else { continue };
        let title = h.title.unwrap_or_default();
        let category = h.category.unwrap_or_default();
        let score = h.score.unwrap_or(0.0);
        if score <= 0.0 {
            continue;
        }
        // Normalize BM25 to ~0-1 by clamping; raw BM25 has unbounded scale.
        let normalized = (score / 4.0).clamp(0.0, 1.0);
        let reason = format!(
            "commit message BM25-matched open {category} \"{}\" (raw score {score:.2})",
            title.chars().take(60).collect::<String>()
        );
        let _ = link::upsert_edge(
            db,
            obs_id,
            &mid,
            "commits_for",
            "system",
            normalized,
            Some(&reason),
        )
        .await;
    }
}

/// Phase 6: emit `obs --touches_file--> memory` edges for every file
/// shared between the observation and an existing memory. Per-file cap
/// of 20 memories so a hot file (e.g. `Cargo.toml`) doesn't link to
/// every memory in the project. Score is `1.0 / matched_count` so each
/// individual edge gets a small contribution and downstream retrieval
/// dampening (Phase 6's 0.3 multiplier) keeps observations from flooding
/// search results.
async fn link_observation_files_to_memories(
    db: &Surreal<Db>,
    obs_id: &RecordId,
    project: &str,
    files: &[String],
) {
    if files.is_empty() {
        return;
    }

    #[derive(Debug, SurrealValue)]
    struct Hit {
        id: Option<RecordId>,
    }

    // One query per file; per-file cap of 20.
    for f in files {
        let trimmed = f.trim();
        if trimmed.is_empty() {
            continue;
        }
        let resp = db
            .query(
                "SELECT id FROM memory \
                 WHERE is_latest = true \
                   AND (project = $project OR project = 'global') \
                   AND $file IN files \
                 LIMIT 20",
            )
            .bind(("project", project.to_string()))
            .bind(("file", trimmed.to_string()))
            .await;
        let mut resp = match resp {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("touches_file query failed for '{trimmed}': {e}");
                continue;
            }
        };
        let hits: Vec<Hit> = resp.take(0).unwrap_or_default();
        if hits.is_empty() {
            continue;
        }
        let n = hits.len() as f64;
        let score = (1.0 / n).clamp(0.05, 1.0);
        for h in hits {
            let Some(mid) = h.id else { continue };
            let reason = format!(
                "observation touched '{trimmed}' (1 of {} memories)",
                n as usize
            );
            let _ = link::upsert_edge(
                db,
                obs_id,
                &mid,
                "touches_file",
                "file",
                score,
                Some(&reason),
            )
            .await;
        }
    }
}

/// Ensure the session record exists, create if not.
async fn ensure_session(db: &Surreal<Db>, payload: &HookPayload) -> Result<()> {
    let sid = format!("session:{}", payload.session_id);
    let mut response = db
        .query("SELECT id FROM type::record($sid)")
        .bind(("sid", sid.clone()))
        .await?;
    let existing: Vec<serde_json::Value> = response.take(0)?;

    if existing.is_empty() {
        db.query(
            "CREATE type::record($sid) SET \
             project = $project, \
             cwd = $cwd, \
             started_at = $started_at, \
             status = 'active', \
             observation_count = 0",
        )
        .bind(("sid", sid.clone()))
        .bind(("project", payload.project.clone()))
        .bind(("cwd", payload.cwd.clone()))
        .bind(("started_at", payload.timestamp.clone()))
        .await?;
    }

    Ok(())
}

// --- Observations search (lifted from web/api.rs::observations_search) ---

/// Search observations with optional filters (session, project, obs_type, time
/// range, importance) and optional BM25 query. Returns the legacy wire shape
/// `{"observations": [...], "count": N}`.
pub async fn search(
    db: &surrealdb::Surreal<crate::db::Db>,
    params: crate::models::ObservationsReq,
) -> anyhow::Result<serde_json::Value> {
    let limit = params.limit.unwrap_or(100);
    let query = params.query.as_deref().unwrap_or("*");

    let mut sql = String::from("SELECT * FROM observation");
    let mut conditions = Vec::new();

    if let Some(ref sid) = params.session_id {
        let sid_clean = sid.strip_prefix("session:").unwrap_or(sid);
        conditions.push(format!(
            "session_id = type::record('session:{}')",
            sid_clean.replace('\'', "")
        ));
    }
    if params.project.is_some() {
        conditions.push("project = $project".to_string());
    }
    if let Some(ref types) = params.obs_type {
        let parts: Vec<String> = types
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| format!("'{}'", s.replace('\'', "")))
            .collect();
        if !parts.is_empty() {
            conditions.push(format!("obs_type IN [{}]", parts.join(", ")));
        }
    }
    if params.since.is_some() {
        conditions.push("timestamp >= $since".to_string());
    }
    if params.until.is_some() {
        conditions.push("timestamp <= $until".to_string());
    }
    if let Some(min_imp) = params.min_importance {
        conditions.push(format!("importance >= {}", min_imp));
    }
    if !query.is_empty() && query != "*" {
        conditions.push("(title @@ $q OR narrative @@ $q)".to_string());
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(&format!(" ORDER BY timestamp DESC LIMIT {limit}"));

    let mut q = db.query(&sql);
    if query != "*" && !query.is_empty() {
        q = q.bind(("q", query.to_string()));
    }
    if let Some(ref project) = params.project {
        q = q.bind(("project", project.clone()));
    }
    if let Some(ref since) = params.since {
        q = q.bind(("since", since.clone()));
    }
    if let Some(ref until) = params.until {
        q = q.bind(("until", until.clone()));
    }

    let mut resp = q.await?;
    let observations: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let count = observations.len();
    Ok(serde_json::json!({"observations": observations, "count": count}))
}
