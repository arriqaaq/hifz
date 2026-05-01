use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use std::sync::Arc;

use crate::client::Client;
use crate::config::Config;
use crate::hash::hash_event;
use crate::jsonl::JsonlReader;
use crate::promote::promote;
use crate::ui::auto_reply;

pub async fn run(cfg: Config) -> Result<()> {
    let project = cfg
        .project
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap().to_string_lossy().to_string());

    let spool = crate::spool::Spool::new(cfg.spool_dir.clone())?;
    let db_path_str = cfg.db_path.to_string_lossy().to_string();
    let client = Client::new(&db_path_str, spool).await?;
    client.drain_spool().await;

    let session_id = uuid::Uuid::new_v4().to_string();
    let _ = client
        .start_session(&json!({
            "sessionId": session_id,
            "project": project,
            "cwd": project,
        }))
        .await;

    let mut child: Child = Command::new(&cfg.pi_bin)
        .arg("--mode")
        .arg("rpc")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn {}", cfg.pi_bin))?;

    let stdin = child.stdin.take().context("pi stdin")?;
    let stdout = child.stdout.take().context("pi stdout")?;

    let stdin = Arc::new(Mutex::new(stdin));
    let mut reader = JsonlReader::new(stdout);

    // Feed an initial prompt if requested.
    if let Some(prompt) = cfg.prompt.clone() {
        write_command(
            &stdin,
            &json!({
                "type": "prompt",
                "id": uuid::Uuid::new_v4().to_string(),
                "text": prompt,
            }),
        )
        .await?;
    }

    let mut sequence: i64 = 0;
    // Run-id is not stamped on ledger rows in v1.1; run linkage lives at the observation level
    // via the existing /observe path. See plan §"T2 — `lookupOpenRun` semantically wrong".
    let current_run_id: Option<String> = None;

    while let Some(line) = reader.next_line().await? {
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("invalid JSONL from pi: {e}");
                continue;
            }
        };
        let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match typ {
            // Protocol records — not agent events.
            "response" => {
                tracing::debug!("rpc response: {v}");
            }
            "extension_ui_request" => {
                let reply = auto_reply(&v, &cfg.ui_mode);
                write_command(&stdin, &reply).await?;
                // Also log the request and our synthesized response to the ledger.
                emit_event(
                    &client,
                    &session_id,
                    current_run_id.as_deref(),
                    &mut sequence,
                    "ui_request",
                    &v,
                )
                .await;
                emit_event(
                    &client,
                    &session_id,
                    current_run_id.as_deref(),
                    &mut sequence,
                    "ui_response",
                    &reply,
                )
                .await;
            }
            // Agent events — drop deltas unless verbose.
            "" => continue,
            _ => {
                if !cfg.verbose_deltas && is_streaming_delta(&v) {
                    continue;
                }
                emit_event(
                    &client,
                    &session_id,
                    current_run_id.as_deref(),
                    &mut sequence,
                    typ,
                    &v,
                )
                .await;

                // Promote selected events.
                if let Some(payload) = promote(typ, &v, &project, &session_id) {
                    client.send_observation(&payload).await;
                }

                if typ == "agent_end" {
                    break;
                }
            }
        }
    }

    let _ = child.wait().await;
    client.end_session(&json!({ "sessionId": session_id })).await;
    Ok(())
}

async fn emit_event(
    client: &Client,
    session_id: &str,
    run_id: Option<&str>,
    sequence: &mut i64,
    event_type: &str,
    evt: &Value,
) {
    *sequence += 1;
    let body = json!({
        "source": "pi_rpc",
        "event_type": event_type,
        "session_id": session_id,
        "run_id": run_id,
        "sequence": *sequence,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "parent_event_id": null,
        "payload_hash": hash_event(session_id, event_type, *sequence, evt),
        "payload": evt,
        "metadata": null,
    });
    client.send_event(&body).await;
}

async fn write_command(stdin: &Arc<Mutex<ChildStdin>>, value: &Value) -> Result<()> {
    let mut s = serde_json::to_string(value)?;
    s.push('\n');
    let mut g = stdin.lock().await;
    g.write_all(s.as_bytes()).await?;
    g.flush().await?;
    Ok(())
}

/// Streaming delta inside a `message_update`. We drop these by default.
fn is_streaming_delta(v: &Value) -> bool {
    if v.get("type").and_then(|t| t.as_str()) != Some("message_update") {
        return false;
    }
    let inner = v
        .get("assistantMessageEvent")
        .and_then(|e| e.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    matches!(
        inner,
        "text_delta"
            | "thinking_delta"
            | "toolcall_delta"
            | "text_start"
            | "thinking_start"
            | "toolcall_start"
    )
}
