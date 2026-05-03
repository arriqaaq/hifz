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

        "hifz_neighbors" => {
            let memory_id = args
                .get("memory_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("memory_id required"))?;
            let mut params = vec![];
            if let Some(rels) = args.get("relations").and_then(|v| v.as_array()) {
                let s = rels
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                if !s.is_empty() {
                    params.push(format!("relations={}", urlencoding::encode(&s)));
                }
            }
            if let Some(h) = args.get("max_hops").and_then(|v| v.as_u64()) {
                params.push(format!("max_hops={h}"));
            }
            let qs = if params.is_empty() {
                String::new()
            } else {
                format!("?{}", params.join("&"))
            };
            state
                .client
                .get(format!(
                    "{}/api/v1/memories/{}/neighbors{qs}",
                    state.base_url,
                    urlencoding::encode(memory_id)
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_backlinks" => {
            let memory_id = args
                .get("memory_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("memory_id required"))?;
            let qs = match args.get("relation").and_then(|v| v.as_str()) {
                Some(r) => format!("?relation={}", urlencoding::encode(r)),
                None => String::new(),
            };
            state
                .client
                .get(format!(
                    "{}/api/v1/memories/{}/backlinks{qs}",
                    state.base_url,
                    urlencoding::encode(memory_id)
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_warmup" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("session_id required"))?;
            let mut params = vec![];
            if let Some(p) = args.get("project").and_then(|v| v.as_str()) {
                params.push(format!("project={}", urlencoding::encode(p)));
            }
            if let Some(n) = args.get("top_n").and_then(|v| v.as_u64()) {
                params.push(format!("top_n={n}"));
            }
            let qs = if params.is_empty() {
                String::new()
            } else {
                format!("?{}", params.join("&"))
            };
            state
                .client
                .get(format!(
                    "{}/api/v1/agent/sessions/{}/warmup{qs}",
                    state.base_url,
                    urlencoding::encode(session_id)
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_project_accumulators" => {
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("project required"))?;
            state
                .client
                .get(format!(
                    "{}/api/v1/projects/{}/accumulators",
                    state.base_url,
                    urlencoding::encode(project)
                ))
                .send()
                .await?
                .json()
                .await?
        }

        "hifz_project_digest" => {
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("project required"))?;
            let days = args.get("days").and_then(|v| v.as_u64()).unwrap_or(30);
            state
                .client
                .get(format!(
                    "{}/api/v1/projects/{}/digest?days={days}",
                    state.base_url,
                    urlencoding::encode(project)
                ))
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
        serde_json::json!({
            "name": "hifz_save",
            "description": "Save a memory to long-term store. Use a typed `category` so retrieval can group/filter by intent. Provide `keywords` and `files` explicitly — they are NOT extracted from the title alone (file paths IN content are auto-detected). For long-form documents (Plan/Design/CodeReview/ShipReport/ContextSlice) put the markdown body in `content_long` and a 1-2 sentence summary in `content`. When LLM enrichment is enabled, the system also generates context_summary, tags, and typed conceptual/argumentative edges with stored reasons.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "content": {"type": "string", "description": "Short retrieval-friendly form. For long-form categories, also provide content_long."},
                    "content_long": {"type": "string", "description": "Optional full markdown body for long-form artifact categories (Plan/Design/CodeReview/ShipReport/ContextSlice). Phase 4 will chunk this for retrieval."},
                    "project": {"type": "string", "description": "Project name (defaults to 'global' if omitted)"},
                    "category": {
                        "type": "string",
                        "enum": [
                            "lesson", "decision", "bug", "fix", "gotcha",
                            "convention", "failure_pattern",
                            "plan", "design", "code_review", "ship_report", "context_slice",
                            "observation", "note"
                        ],
                        "default": "note",
                        "description": "Typed category. Long-form: plan/design/code_review/ship_report/context_slice. Lifecycle pairs: bug↔fix (use closes_memory_id)."
                    },
                    "keywords": {"type": "array", "items": {"type": "string"}, "description": "Caller-supplied salient terms. NOT extracted from text — be explicit. Lowercase preferred."},
                    "files": {"type": "array", "items": {"type": "string"}, "description": "Caller-supplied file paths. Server also regex-extracts file paths from content."},
                    "tags": {"type": "array", "items": {"type": "string"}, "description": "Optional coarse buckets (e.g. \"auth\", \"perf\"). LLM enrichment may add more."},
                    "closes_memory_id": {"type": "string", "description": "When this memory closes/resolves another (e.g. fix→bug). Writes a `closes` edge."},
                    "supersedes_memory_id": {"type": "string", "description": "When this memory replaces another. Writes `supersedes` edge AND marks the old one is_latest=false."},
                    "session_id": {"type": "string", "description": "Session ID for provenance tracking"}
                },
                "required": ["title", "content"]
            }
        }),
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
        // Phase 9: typed graph + project surface tools.
        serde_json::json!({"name": "hifz_neighbors", "description": "Walk the typed graph from a memory. Use this to find related memories by specific relation kinds: conceptual (related/broader/narrower), argumentative (supports/contradicts/elaborates), lifecycle (closes/supersedes), or co-occurrence (co_occurs_*).", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string", "description": "RecordId like 'memory:abc'"}, "relations": {"type": "array", "items": {"type": "string", "enum": ["co_occurs_files", "co_occurs_keywords", "co_occurs_embedding", "mentions", "generated_by", "informed_by", "derived_from", "attributed_to", "part_of", "follows", "broader", "narrower", "related", "same_as", "supports", "contradicts", "elaborates", "responds_to", "supersedes", "closes", "touches_file", "commits_for", "tests"]}, "description": "Filter to specific typed relations. Empty = all relations."}, "max_hops": {"type": "integer", "default": 1, "minimum": 1, "maximum": 4}}, "required": ["memory_id"]}}),
        serde_json::json!({"name": "hifz_backlinks", "description": "List incoming edges for a memory — every memory or observation that references it. Optional relation filter.", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string"}, "relation": {"type": "string", "description": "Optional: limit to one typed relation"}}, "required": ["memory_id"]}}),
        serde_json::json!({"name": "hifz_warmup", "description": "Build the project-scoped warmup digest — latest plan, recent decisions, conventions, open bugs, gotchas, failure patterns, recent lessons. Inject the `top` field as system context at session start to give the agent a 'here's where you are' snapshot.", "inputSchema": {"type": "object", "properties": {"session_id": {"type": "string"}, "project": {"type": "string", "description": "Defaults to the session's project."}, "top_n": {"type": "integer", "default": 15}}, "required": ["session_id"]}}),
        serde_json::json!({"name": "hifz_project_accumulators", "description": "Get the project's cross-cutting rollup: latest plan, decisions, conventions, open bugs, gotchas, failure patterns, recent lessons. Like hifz_warmup but project-only (no session scope).", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}}),
        serde_json::json!({"name": "hifz_project_digest", "description": "Chronological digest of recent activity for a project, grouped by typed category. Powers a 'what happened in the last N days' view.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "days": {"type": "integer", "default": 30, "minimum": 1}}, "required": ["project"]}}),
    ]
}
