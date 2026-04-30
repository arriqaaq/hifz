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

    // Store in SurrealDB
    let session_rid = format!("session:{}", payload.session_id);
    let sql = "CREATE observation SET \
        session_id = type::record($session_rid), \
        timestamp = $timestamp, \
        obs_type = $obs_type, \
        title = $title, \
        subtitle = $subtitle, \
        facts = $facts, \
        facts_text = $facts_text, \
        narrative = $narrative, \
        keywords = $keywords, \
        files = $files, \
        importance = $importance, \
        confidence = $confidence, \
        embedding = $embedding";

    #[derive(Debug, SurrealValue)]
    struct Created {
        id: Option<RecordId>,
    }

    let sql_with_return = format!("{sql} RETURN id");
    let response = db
        .query(&sql_with_return)
        .bind(("session_rid", session_rid))
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
    }

    // Increment session observation count
    db.query("UPDATE type::record($sid) SET observation_count += 1")
        .bind(("sid", format!("session:{}", payload.session_id)))
        .await?;

    // When adapter sends a commit_made observation, trigger grounding and causal edge
    if compressed.obs_type == "commit_made" {
        let _ = ground::on_commit_observation(db, &payload.project, &compressed.files).await;
        if let Some(obs_id) = new_obs_id.as_ref() {
            if let Ok(Some(run_id)) = run::find_open(db, &payload.session_id).await {
                let _ = link::upsert_edge(db, obs_id, &run_id, "generated_by", "system", 1.0).await;
            }
        }
    }

    Ok(Some(compressed.title))
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
