pub mod http;
pub mod tools;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP server state — thin HTTP proxy to the REST server.
/// No embedded SurrealDB, no fastembed, no direct DB access.
#[derive(Clone)]
pub struct McpState {
    pub client: reqwest::Client,
    pub base_url: String,
}

/// Best-effort raw-wire diagnostic: append a `tools/call` line exactly as it
/// arrived on stdin to `~/.hifz/logs/mcp-wire.log`. This is ground truth for
/// "what did the Claude Code client actually send" — used to settle whether an
/// empty-args failure is a client drop, a stringified-args interop quirk we
/// mis-parse, or the model emitting no arguments. Never fails the request.
pub(crate) fn wire_append(tag: &str, msg: &str) {
    use std::io::Write;
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let dir = std::path::Path::new(&home).join(".hifz/logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("mcp-wire.log"))
    {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        // Truncate very large payloads (big content_long) — the args *shape*
        // is what matters for the diagnosis, not the full body.
        let preview: String = msg.chars().take(4096).collect();
        let _ = writeln!(f, "{ts} {tag} {preview}");
    }
}

/// Run the MCP server over stdio (JSON-RPC 2.0, line-delimited).
/// Proxies all tool calls to the REST server via HTTP.
pub async fn serve_stdio(state: McpState) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let request: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let error_resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {e}")}
                });
                let mut out = serde_json::to_string(&error_resp)?;
                out.push('\n');
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
                continue;
            }
        };

        let id = request.get("id").cloned();
        let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = request
            .get("params")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        // Notifications (no id) get no response
        if id.is_none() || id.as_ref().map(|v| v.is_null()).unwrap_or(false) {
            continue;
        }

        let result = match method {
            "initialize" => handle_initialize(),
            "initialized" => continue,
            "tools/list" => tools::list_tools(),
            "tools/call" => {
                wire_append("RAW", &line);
                tools::call_tool(&state, &params).await
            }
            "resources/list" => handle_resources_list(),
            "prompts/list" => handle_prompts_list(),
            _ => Err(anyhow::anyhow!("Unknown method: {method}")),
        };

        let response = match result {
            Ok(value) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": value,
            }),
            Err(e) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": http::rpc_code(&e), "message": e.to_string()},
            }),
        };

        let mut out = serde_json::to_string(&response)?;
        out.push('\n');
        stdout.write_all(out.as_bytes()).await?;
        stdout.flush().await?;
    }

    Ok(())
}

fn handle_initialize() -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {},
            "resources": {},
            "prompts": {},
        },
        "serverInfo": {
            "name": "hifz",
            "version": env!("CARGO_PKG_VERSION"),
        }
    }))
}

fn handle_resources_list() -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "resources": [
            {"uri": "hifz://status", "name": "Status", "description": "Health and stats"},
            {"uri": "hifz://latest", "name": "Latest", "description": "Latest 10 memories"},
        ]
    }))
}

fn handle_prompts_list() -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "prompts": [
            {"name": "recall_context", "description": "Search and return context"},
        ]
    }))
}
