use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::compress::{compress_llm, compress_synthetic};
use crate::db::Db;
use crate::dedup::DedupMap;
use crate::embed::Embedder;
use crate::episode;
use crate::models::HookPayload;
use crate::ollama::OllamaClient;

/// Capture a raw observation from a Claude Code hook.
/// Deduplicates, compresses, embeds, and stores.
pub async fn observe(
    db: &Surreal<Db>,
    dedup: &DedupMap,
    embedder: &Embedder,
    ollama: Option<&OllamaClient>,
    auto_compress: bool,
    payload: HookPayload,
) -> Result<Option<String>> {
    // Episode lifecycle — fire before dedup so lifecycle events aren't dropped.
    // On UserPromptSubmit start a new episode; on Stop/TaskCompleted close the open one.
    match payload.hook_type.as_str() {
        "UserPromptSubmit" | "prompt_submit" => {
            let prompt = payload
                .data
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ensure_session(db, &payload).await?;
            if let Some(session_rid) = session_record_id(db, &payload.session_id).await {
                let _ = episode::start(db, &session_rid, &payload.project, &prompt).await;
            }
        }
        "Stop" | "stop" | "TaskCompleted" | "task_completed" => {
            if let Some(open) = latest_open_episode(db, &payload.session_id)
                .await
                .ok()
                .flatten()
            {
                let _ = episode::close(db, &open, "success", None).await;
            }
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

    // Generate embedding
    let embed_text = format!("{} {}", compressed.title, compressed.narrative);
    let embedding = match embedder.embed_single(&embed_text) {
        Ok(vec) => Some(vec),
        Err(e) => {
            tracing::warn!("Embedding failed: {e}");
            None
        }
    };

    let facts_text = if compressed.facts.is_empty() {
        None
    } else {
        Some(compressed.facts.join(" "))
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
        concepts = $concepts, \
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
        .bind(("concepts", compressed.concepts.clone()))
        .bind(("files", compressed.files.clone()))
        .bind(("importance", compressed.importance))
        .bind(("confidence", compressed.confidence))
        .bind(("embedding", embedding.clone()))
        .await?;
    let mut response = response.check()?;
    let created: Vec<Created> = response.take(0).unwrap_or_default();
    let new_obs_id = created.into_iter().next().and_then(|c| c.id);

    // Append to the open episode (if any) for this session.
    if let Some(obs_id) = new_obs_id.as_ref() {
        if let Ok(Some(ep)) = latest_open_episode(db, &payload.session_id).await {
            let _ = episode::append(db, &ep, obs_id).await;
        }
    }

    // Increment session observation count
    db.query("UPDATE type::record($sid) SET observation_count += 1")
        .bind(("sid", format!("session:{}", payload.session_id)))
        .await?;

    Ok(Some(compressed.title))
}

/// Resolve "session:<id>" into a `RecordId` by round-tripping through SurrealQL,
/// matching the existing `type::record($sid)` binding pattern used elsewhere.
async fn session_record_id(db: &Surreal<Db>, session_id: &str) -> Option<RecordId> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let sid = format!("session:{}", session_id);
    let mut resp = db
        .query("SELECT id FROM type::record($sid)")
        .bind(("sid", sid))
        .await
        .ok()?;
    let rows: Vec<Row> = resp.take(0).ok()?;
    rows.into_iter().next().and_then(|r| r.id)
}

/// Look up the most recent open episode for a session (ended_at IS NONE).
async fn latest_open_episode(db: &Surreal<Db>, session_id: &str) -> Result<Option<RecordId>> {
    let Some(sid) = session_record_id(db, session_id).await else {
        return Ok(None);
    };
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query(
            "SELECT id FROM episode \
             WHERE session_id = $sid AND ended_at IS NONE \
             ORDER BY started_at DESC LIMIT 1",
        )
        .bind(("sid", sid))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    Ok(rows.into_iter().next().and_then(|r| r.id))
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
