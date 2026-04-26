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
            let session_id = args.get("session_id").and_then(|v| v.as_str());
            let mut body = serde_json::json!({"query": query, "limit": limit});
            if let Some(p) = project {
                body["project"] = serde_json::Value::String(p.to_string());
            }
            if let Some(sid) = session_id {
                body["session_id"] = serde_json::Value::String(sid.to_string());
            }
            state
                .client
                .post(format!("{}/api/v1/search/agentic", state.base_url))
                .json(&body)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_save" => {
            state
                .client
                .post(format!("{}/api/v1/memories", state.base_url))
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
                .get(format!(
                    "{}/api/v1/agent/sessions?limit={limit}",
                    state.base_url
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_digest" => {
            let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
            state
                .client
                .get(format!(
                    "{}/api/v1/agent/digest?project={project}",
                    state.base_url
                ))
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
                    "{}/api/v1/agent/timeline?session_id={session_id}&limit={limit}",
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
                .get(format!("{}/api/v1/export", state.base_url))
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
                .get(format!("{}/api/v1/core/{project}", state.base_url))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_core_edit" => {
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .unwrap_or("global");
            state
                .client
                .patch(format!("{}/api/v1/core/{project}", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_runs" => {
            state
                .client
                .post(format!("{}/api/v1/agent/runs", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_commits" => {
            let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
            let branch = args.get("branch").and_then(|v| v.as_str());
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
            let sha = args.get("sha").and_then(|v| v.as_str());

            let mut url = format!(
                "{}/api/v1/agent/commits?project={project}&limit={limit}",
                state.base_url
            );
            if let Some(b) = branch {
                url.push_str(&format!("&branch={b}"));
            }
            if let Some(s) = sha {
                url.push_str(&format!("&sha={s}"));
            }

            state.client.get(&url).send().await?.json().await?
        }

        "hifz_evolve" => {
            let memory_id = args.get("memory_id").and_then(|v| v.as_str()).unwrap_or("");
            let id = memory_id.strip_prefix("memory:").unwrap_or(memory_id);
            state
                .client
                .post(format!("{}/api/v1/memories/{id}/evolve", state.base_url))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_delete" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            state
                .client
                .delete(format!("{}/api/v1/memories", state.base_url))
                .json(&serde_json::json!({"id": id}))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_current_plan" => {
            let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
            state
                .client
                .get(format!(
                    "{}/api/v1/agent/plans/current?project={project}",
                    state.base_url
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_plans" => {
            let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
            let status = args.get("status").and_then(|v| v.as_str()).unwrap_or("all");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
            state
                .client
                .get(format!(
                    "{}/api/v1/agent/plans?project={project}&status={status}&limit={limit}",
                    state.base_url
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_complete_plan" => {
            let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
            let plan: serde_json::Value = state
                .client
                .get(format!(
                    "{}/api/v1/agent/plans/current?project={project}",
                    state.base_url
                ))
                .send()
                .await?
                .json()
                .await?;

            if let Some(plan_id) = plan.get("id").and_then(|v| v.as_str()) {
                let id = plan_id.strip_prefix("plan:").unwrap_or(plan_id);
                state
                    .client
                    .post(format!(
                        "{}/api/v1/agent/plans/{id}/complete",
                        state.base_url
                    ))
                    .json(&serde_json::json!({}))
                    .send()
                    .await?
                    .json()
                    .await?
            } else {
                serde_json::json!({"status": "no_active_plan"})
            }
        }

        "hifz_activate_plan" => {
            state
                .client
                .post(format!("{}/api/v1/agent/plans/activate", state.base_url))
                .json(&args)
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_trace" => {
            state
                .client
                .post(format!("{}/api/v1/trace", state.base_url))
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
        serde_json::json!({"name": "hifz_recall", "description": "Search past observations and memories with graph expansion (optionally project-scoped)", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "project": {"type": "string"}, "session_id": {"type": "string", "description": "Session ID for provenance tracking"}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_save", "description": "Save an insight, decision, or pattern to long-term memory (project-scoped)", "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "content": {"type": "string"}, "project": {"type": "string", "description": "Project name (defaults to 'global' if omitted)"}, "category": {"type": "string", "enum": ["pattern", "preference", "architecture", "bug", "workflow", "fact"]}, "keywords": {"type": "array", "items": {"type": "string"}}, "files": {"type": "array", "items": {"type": "string"}}, "session_id": {"type": "string", "description": "Session ID for provenance tracking"}}, "required": ["title", "content"]}}),
        serde_json::json!({"name": "hifz_search", "description": "Hybrid semantic + keyword search with RRF fusion and graph expansion (optionally project-scoped)", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "project": {"type": "string"}, "session_id": {"type": "string", "description": "Session ID for provenance tracking"}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_sessions", "description": "List recent sessions", "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "default": 20}}}}),
        serde_json::json!({"name": "hifz_digest", "description": "Get project intelligence — top keywords, files, and stats", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_timeline", "description": "Chronological observations", "inputSchema": {"type": "object", "properties": {"session_id": {"type": "string"}, "limit": {"type": "integer", "default": 50}}}}),
        serde_json::json!({"name": "hifz_export", "description": "Export all memory data", "inputSchema": {"type": "object", "properties": {}}}),
        serde_json::json!({"name": "hifz_delete", "description": "Delete a memory by ID", "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}}),
        serde_json::json!({"name": "hifz_core_get", "description": "Read the always-on core memory block for a project (identity, goals, invariants, watchlist)", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_core_edit", "description": "Edit the always-on core memory block. field=identity|goals|invariants|watchlist, op=set|add|remove", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "field": {"type": "string", "enum": ["identity", "goals", "invariants", "watchlist"]}, "op": {"type": "string", "enum": ["set", "add", "remove"]}, "value": {"type": "string"}}, "required": ["project", "field", "op", "value"]}}),
        serde_json::json!({"name": "hifz_runs", "description": "Search past task-scoped runs (prompt + derived lesson) via hybrid BM25 fusion", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}, "limit": {"type": "integer", "default": 10}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_commits", "description": "List recent git commits for a project. Use this to see repo history and continue from a specific point.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "branch": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "sha": {"type": "string", "description": "Get a specific commit by SHA"}}}}),
        serde_json::json!({"name": "hifz_evolve", "description": "Run A-MEM Memory Evolution on a memory — LLM refines neighbour tags/context/links (requires HIFZ_LLM_EVOLVE=true and Ollama)", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string", "description": "RecordId like 'memory:xyz'"}}, "required": ["memory_id"]}}),
        serde_json::json!({"name": "hifz_current_plan", "description": "Get the currently active plan for this project. Returns null if no active plan.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_plans", "description": "List plans for a project. Filter by status (active, completed, abandoned, all).", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "status": {"type": "string", "enum": ["active", "completed", "abandoned", "all"], "default": "all"}, "limit": {"type": "integer", "default": 10}}}}),
        serde_json::json!({"name": "hifz_complete_plan", "description": "Mark the current active plan as completed.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_activate_plan", "description": "Activate a plan for this session. Adds plan title to core memory goals and files to watchlist. The plan will then be injected into context.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "plan_id": {"type": "string", "description": "Optional. If omitted, activates the most recent active plan."}, "session_id": {"type": "string", "description": "Session ID for provenance tracking"}}}}),
        serde_json::json!({"name": "hifz_trace", "description": "Trace the knowledge graph from a starting node. Returns nodes and edges showing provenance, similarity, and causal relationships.", "inputSchema": {"type": "object", "properties": {"id": {"type": "string", "description": "Starting node ID (e.g. 'memory:abc', 'run:xyz')"}, "direction": {"type": "string", "enum": ["forward", "backward", "both"], "default": "both"}, "relations": {"type": "array", "items": {"type": "string"}, "description": "Filter to specific relation types"}, "max_hops": {"type": "integer", "default": 2}}, "required": ["id"]}}),
    ]
}
