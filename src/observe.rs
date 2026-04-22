use std::path::Path;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::commit;
use crate::compress::{compress_llm, compress_synthetic};
use crate::db::Db;
use crate::dedup::DedupMap;
use crate::embed::Embedder;
use crate::git_detect;
use crate::ground;
use crate::models::HookPayload;
use crate::ollama::OllamaClient;
use crate::run;

/// Capture a raw observation from a Claude Code hook.
/// Deduplicates, compresses, embeds, and stores.
pub async fn observe(
    db: &Surreal<Db>,
    dedup: &DedupMap,
    embedder: &Embedder,
    ollama: Option<&OllamaClient>,
    auto_compress: bool,
    payload: HookPayload,
    git_path: Option<&Path>,
) -> Result<Option<String>> {
    // Run lifecycle — fire before dedup so lifecycle events aren't dropped.
    // Runs are task-scoped: UserPromptSubmit appends to open run or starts new.
    // TaskCompleted/Stop close the run.
    match payload.hook_type.as_str() {
        "UserPromptSubmit" | "prompt_submit" => {
            let prompt = payload
                .data
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ensure_session(db, &payload).await?;

            // Check if run already open for this session
            if let Some(open_run) = latest_open_run(db, &payload.session_id)
                .await
                .ok()
                .flatten()
            {
                // Append prompt to existing run (don't close)
                let _ = run::append_prompt(db, &open_run, &prompt).await;
            } else {
                // No open run — start new one
                if let Some(session_rid) = session_record_id(db, &payload.session_id).await {
                    if let Ok(Some(run_id)) =
                        run::start(db, &session_rid, &payload.project, &prompt).await
                    {
                        // Link to active plan if one exists
                        if let Ok(Some(active_plan)) =
                            crate::plan::get_active(db, &payload.project).await
                        {
                            if let Some(plan_id) = active_plan.id.as_ref() {
                                let _ = run::set_plan(db, &run_id, plan_id).await;
                            }
                        }
                    }
                }
            }
        }
        "Stop" | "stop" | "TaskCompleted" | "task_completed" => {
            if let Some(open) = latest_open_run(db, &payload.session_id)
                .await
                .ok()
                .flatten()
            {
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

    // Generate embedding — include facts, concepts, files for richer vectors
    let mut embed_text = format!("{}\n{}", compressed.title, compressed.narrative);
    if let Some(ref ft) = facts_text {
        embed_text.push_str("\nfacts: ");
        embed_text.push_str(ft);
    }
    if !compressed.concepts.is_empty() {
        embed_text.push_str("\nconcepts: ");
        embed_text.push_str(&compressed.concepts.join(", "));
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

    // Append to the open run (if any) for this session.
    if let Some(obs_id) = new_obs_id.as_ref() {
        match latest_open_run(db, &payload.session_id).await {
            Ok(Some(r)) => {
                if let Err(e) = run::append(db, &r, obs_id).await {
                    tracing::warn!("run::append failed: {e}");
                }
            }
            Ok(None) => {
                tracing::debug!("no open run for session {}", payload.session_id);
            }
            Err(e) => {
                tracing::warn!("latest_open_run lookup failed: {e}");
            }
        }
    }

    // Increment session observation count
    db.query("UPDATE type::record($sid) SET observation_count += 1")
        .bind(("sid", format!("session:{}", payload.session_id)))
        .await?;

    // Detect git commits from Bash tool output
    tracing::debug!(
        tool_name = tool_name,
        hook_type = %payload.hook_type,
        has_tool_input = payload.data.get("tool_input").or_else(|| payload.data.get("toolInput")).is_some(),
        has_tool_output = payload.data.get("tool_output").or_else(|| payload.data.get("toolOutput")).is_some(),
        "git_detect: checking observation"
    );
    if tool_name == "Bash" || tool_name == "Shell" {
        let command_str = payload
            .data
            .get("tool_input")
            .or_else(|| payload.data.get("toolInput"))
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        let output_str = payload
            .data
            .get("tool_output")
            .or_else(|| payload.data.get("toolOutput"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        tracing::info!(
            command = command_str,
            output_len = output_str.len(),
            output_preview = &output_str[..output_str.len().min(200)],
            "git_detect: Bash tool, checking for commit"
        );
        let detected = git_detect::detect_commit(&payload.data);
        tracing::info!(detected = detected.is_some(), "git_detect: result");
        if let Some(detected) = detected {
            let session_rid = session_record_id(db, &payload.session_id).await;
            let run_rid = latest_open_run(db, &payload.session_id)
                .await
                .ok()
                .flatten();
            let data = commit::CommitData {
                sha: detected.sha,
                message: detected.message,
                author: String::new(),
                branch: detected.branch,
                project: payload.project.clone(),
                files_changed: vec![],
                insertions: None,
                deletions: None,
                is_amend: false,
                timestamp: payload.timestamp.clone(),
            };
            if let Err(e) = commit::record_commit(db, data, session_rid, run_rid, git_path).await {
                tracing::warn!("commit recording failed: {e}");
            }
        }
    }

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
    let resp = db
        .query("SELECT id FROM type::record($sid)")
        .bind(("sid", sid.clone()))
        .await;
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("session_record_id query failed for {sid}: {e}");
            return None;
        }
    };
    let rows: Vec<Row> = resp.take(0).ok()?;
    let result = rows.into_iter().next().and_then(|r| r.id);
    if result.is_none() {
        tracing::debug!("session_record_id: no record found for {sid}");
    }
    result
}

/// Look up the most recent open run for a session (ended_at IS NONE).
async fn latest_open_run(db: &Surreal<Db>, session_id: &str) -> Result<Option<RecordId>> {
    let Some(sid) = session_record_id(db, session_id).await else {
        return Ok(None);
    };
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
        started_at: Option<String>,
    }
    let mut resp = db
        .query(
            "SELECT id, started_at FROM run \
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
