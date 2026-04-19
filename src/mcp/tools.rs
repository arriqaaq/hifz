use anyhow::Result;

use crate::mcp::McpState;

/// List all available MCP tools.
pub fn list_tools() -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "tools": tool_defs()
    }))
}

/// Dispatch a tool call — proxies to the REST server via HTTP.
pub async fn call_tool(state: &McpState, params: &serde_json::Value) -> Result<serde_json::Value> {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let result: serde_json::Value = match name {
        "hifz_recall" | "hifz_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
            let project = args.get("project").and_then(|v| v.as_str());
            let mut body = serde_json::json!({"query": query, "limit": limit});
            if let Some(p) = project {
                body["project"] = serde_json::Value::String(p.to_string());
            }
            state
                .client
                .post(format!("{}/hifz/smart-search", state.base_url))
                .json(&body)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_save" => {
            state
                .client
                .post(format!("{}/hifz/remember", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_sessions" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
            state
                .client
                .get(format!("{}/hifz/sessions?limit={limit}", state.base_url))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_digest" => {
            let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
            state
                .client
                .get(format!("{}/hifz/digest?project={project}", state.base_url))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_timeline" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50);
            state
                .client
                .get(format!(
                    "{}/hifz/timeline?session_id={session_id}&limit={limit}",
                    state.base_url
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_export" => {
            state
                .client
                .get(format!("{}/hifz/export", state.base_url))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_core_get" => {
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .unwrap_or("global");
            state
                .client
                .get(format!("{}/hifz/core?project={project}", state.base_url))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_core_edit" => {
            state
                .client
                .post(format!("{}/hifz/core/edit", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_episodes" => {
            state
                .client
                .post(format!("{}/hifz/episodes", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_evolve" => {
            state
                .client
                .post(format!("{}/hifz/evolve", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_delete" => {
            state
                .client
                .post(format!("{}/hifz/forget", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        _ => {
            return Err(anyhow::anyhow!("Unknown tool: {name}"));
        }
    };

    Ok(serde_json::json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(&result)?}]
    }))
}

fn tool_defs() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"name": "hifz_recall", "description": "Search past observations and memories (optionally project-scoped)", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "project": {"type": "string"}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_save", "description": "Save an insight, decision, or pattern to long-term memory (project-scoped)", "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "content": {"type": "string"}, "project": {"type": "string", "description": "Project name (defaults to 'global' if omitted)"}, "type": {"type": "string", "enum": ["pattern", "preference", "architecture", "bug", "workflow", "fact"]}, "concepts": {"type": "array", "items": {"type": "string"}}, "files": {"type": "array", "items": {"type": "string"}}}, "required": ["title", "content"]}}),
        serde_json::json!({"name": "hifz_search", "description": "Hybrid semantic + keyword search with RRF fusion (optionally project-scoped)", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "project": {"type": "string"}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_sessions", "description": "List recent sessions", "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "default": 20}}}}),
        serde_json::json!({"name": "hifz_digest", "description": "Get project intelligence — top concepts, files, and stats", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_timeline", "description": "Chronological observations", "inputSchema": {"type": "object", "properties": {"session_id": {"type": "string"}, "limit": {"type": "integer", "default": 50}}}}),
        serde_json::json!({"name": "hifz_export", "description": "Export all memory data", "inputSchema": {"type": "object", "properties": {}}}),
        serde_json::json!({"name": "hifz_delete", "description": "Delete a memory by ID", "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}}),
        serde_json::json!({"name": "hifz_core_get", "description": "Read the always-on core memory block for a project (identity, goals, invariants, watchlist)", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_core_edit", "description": "Edit the always-on core memory block. field=identity|goals|invariants|watchlist, op=set|add|remove", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "field": {"type": "string", "enum": ["identity", "goals", "invariants", "watchlist"]}, "op": {"type": "string", "enum": ["set", "add", "remove"]}, "value": {"type": "string"}}, "required": ["project", "field", "op", "value"]}}),
        serde_json::json!({"name": "hifz_episodes", "description": "Search past task-scoped episodes (prompt + derived lesson) via hybrid BM25 fusion", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}, "limit": {"type": "integer", "default": 10}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_evolve", "description": "Run A-MEM Memory Evolution on a memory id — LLM refines neighbour tags/context/links (requires HIFZ_LLM_EVOLVE=true and Ollama)", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string", "description": "RecordId like 'hifz:xyz'"}}, "required": ["memory_id"]}}),
    ]
}
