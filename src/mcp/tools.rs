use anyhow::Result;

use crate::mcp::McpState;
use crate::mcp::http::RequestBuilderExt;

/// Tools advertised to MCP clients — a deliberately small "model-intent" core
/// (recall/save/search/delete, sessions, the plan lifecycle, code grounding).
///
/// The other ~17 tools (analytics/admin/graph/context-injection) stay fully
/// functional: dispatch (`call_tool`) and `validate_required` keep using the
/// FULL `tool_defs()`, and everything is reachable via the REST API /
/// dashboard. They are simply not *advertised*, so the advertised schema stays
/// small (context-bloat reduction + the recognised hybrid best practice).
///
/// This is NOT a fix for the Claude Code empty-arguments arg-drop
/// (`anthropics/claude-code#3966`-class): that is a verified *client* bug,
/// reproducible with a single tool. The serialization-proof save path is the
/// `hifz save` CLI (`Command::Save`) → `POST /api/v1/memories`.
const CORE_TOOLS: &[&str] = &[
    "hifz_recall",
    "hifz_save",
    "hifz_search",
    "hifz_delete",
    "hifz_sessions",
    "hifz_current_plan",
    "hifz_plans",
    "hifz_activate_plan",
    "hifz_complete_plan",
    "hifz_code_index",
    "hifz_code_search",
    "hifz_link_code",
    "hifz_link_symbol",
];

/// List the advertised MCP tools (the `CORE_TOOLS` allowlist). Atlas tools are
/// never core. Hidden tools remain dispatchable — see `CORE_TOOLS`.
pub fn list_tools() -> Result<serde_json::Value> {
    let tools: Vec<serde_json::Value> = tool_defs()
        .into_iter()
        .filter(|t| {
            t.get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|n| CORE_TOOLS.contains(&n))
        })
        .collect();
    Ok(serde_json::json!({ "tools": tools }))
}

#[cfg(feature = "atlas")]
fn atlas_tool_defs() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"name": "atlas_ingest", "description": "Ingest PDFs/markdown/txt under a path into the atlas corpus graph", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}),
        serde_json::json!({"name": "atlas_code", "description": "Project a repo's code graph (scope-qualified symbols + calls/imports/contains with resolution) into atlas", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}),
        serde_json::json!({"name": "atlas_extract", "description": "LLM concept-graph extraction over ingested docs (no-LLM embedding fallback if no backend)", "inputSchema": {"type": "object", "properties": {}}}),
        serde_json::json!({"name": "atlas_cluster", "description": "Modularity clustering + >25% re-split over the atlas graph", "inputSchema": {"type": "object", "properties": {}}}),
        serde_json::json!({"name": "atlas_insights", "description": "Hub nodes / surprising cross-cluster links / isolated nodes (JSON)", "inputSchema": {"type": "object", "properties": {}}}),
        serde_json::json!({"name": "atlas_query", "description": "Hybrid text query over the atlas corpus graph", "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}}),
    ]
}

/// Dispatch a tool call — proxies to the REST server via HTTP.
pub async fn call_tool(state: &McpState, params: &serde_json::Value) -> Result<serde_json::Value> {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");

    // Robust arg extraction. The MCP spec says `params.arguments` is an
    // object, but clients have been observed to double-encode it as a JSON
    // *string*; naive `.unwrap_or({})` would then make `validate_required`
    // report a misleading "Received keys: []" even though args WERE sent.
    // Tolerate the string form, and record the actual shape to the wire log
    // so an empty-args failure's cause is *evidenced*, not assumed
    // (client-drop vs stringified-quirk vs model-sent-nothing).
    let (args, shape) = match params.get("arguments") {
        None => (serde_json::json!({}), "absent"),
        Some(serde_json::Value::Null) => (serde_json::json!({}), "null"),
        Some(serde_json::Value::String(s)) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v) if v.is_object() => (v, "string->object(recovered)"),
            _ => (serde_json::json!({}), "string(unparseable)"),
        },
        Some(v) if v.is_object() => (v.clone(), "object"),
        Some(v) => (v.clone(), "non-object"),
    };
    crate::mcp::wire_append("SHAPE", &format!("name={name} arguments={shape}"));

    // One uniform, self-correcting `-32602` for every tool's required args —
    // fails fast before the HTTP round-trip, no per-tool guards.
    validate_required(name, &args)?;

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
                .send_decode()
                .await?
        }

        "hifz_save" => {
            // `content` is the only required field (enforced uniformly by
            // `validate_required`); `title` is optional and derived
            // server-side from `content` when omitted. Forward verbatim.
            state
                .client
                .post(format!("{}/api/v1/memories", state.base_url))
                .json(&args)
                .send_decode()
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
                .send_decode()
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
                .send_decode()
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
                .send_decode()
                .await?
        }

        "hifz_timeline_causal" => {
            let mut qs = String::new();
            if let Some(s) = args.get("seed").and_then(|v| v.as_str()) {
                qs.push_str(&format!("seed={s}&"));
            }
            if let Some(p) = args.get("project").and_then(|v| v.as_str()) {
                qs.push_str(&format!("project={p}&"));
            }
            let max_hops = args.get("max_hops").and_then(|v| v.as_u64()).unwrap_or(3);
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
            state
                .client
                .get(format!(
                    "{}/api/v1/agent/timeline/causal?{qs}max_hops={max_hops}&limit={limit}",
                    state.base_url
                ))
                .send_decode()
                .await?
        }

        "hifz_export" => {
            state
                .client
                .get(format!("{}/api/v1/export", state.base_url))
                .send_decode()
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
                .send_decode()
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
                .send_decode()
                .await?
        }

        "hifz_runs" => {
            state
                .client
                .post(format!("{}/api/v1/agent/runs", state.base_url))
                .json(&args)
                .send_decode()
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

            state.client.get(&url).send_decode().await?
        }

        "hifz_evolve" => {
            let memory_id = args.get("memory_id").and_then(|v| v.as_str()).unwrap_or("");
            let id = memory_id.strip_prefix("memory:").unwrap_or(memory_id);
            state
                .client
                .post(format!("{}/api/v1/memories/{id}/evolve", state.base_url))
                .send_decode()
                .await?
        }

        "hifz_delete" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            state
                .client
                .delete(format!("{}/api/v1/memories", state.base_url))
                .json(&serde_json::json!({"id": id}))
                .send_decode()
                .await?
        }

        "hifz_view" => {
            let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let id = id.strip_prefix("memory:").unwrap_or(id);
            state
                .client
                .get(format!("{}/api/v1/memories/{id}/view", state.base_url))
                .send_decode()
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
                .send_decode()
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
                .send_decode()
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
                .send_decode()
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
                    .send_decode()
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
                .send_decode()
                .await?
        }

        "hifz_trace" => {
            state
                .client
                .post(format!("{}/api/v1/trace", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }

        "hifz_neighbors" => {
            let memory_id = args
                .get("memory_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::mcp::http::ProxyError::BadArgs("memory_id required".into())
                })?;
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
                .send_decode()
                .await?
        }

        "hifz_backlinks" => {
            let memory_id = args
                .get("memory_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::mcp::http::ProxyError::BadArgs("memory_id required".into())
                })?;
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
                .send_decode()
                .await?
        }

        "hifz_warmup" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    crate::mcp::http::ProxyError::BadArgs("session_id required".into())
                })?;
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
                .send_decode()
                .await?
        }

        "hifz_project_accumulators" => {
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::mcp::http::ProxyError::BadArgs("project required".into()))?;
            state
                .client
                .get(format!(
                    "{}/api/v1/projects/{}/accumulators",
                    state.base_url,
                    urlencoding::encode(project)
                ))
                .send_decode()
                .await?
        }

        "hifz_project_digest" => {
            let project = args
                .get("project")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::mcp::http::ProxyError::BadArgs("project required".into()))?;
            let days = args.get("days").and_then(|v| v.as_u64()).unwrap_or(30);
            state
                .client
                .get(format!(
                    "{}/api/v1/projects/{}/digest?days={days}",
                    state.base_url,
                    urlencoding::encode(project)
                ))
                .send_decode()
                .await?
        }

        #[cfg(feature = "code")]
        "hifz_code_index" => {
            state
                .client
                .post(format!("{}/api/v1/code/index", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }

        #[cfg(feature = "code")]
        "hifz_code_search" => {
            state
                .client
                .post(format!("{}/api/v1/code/search", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }

        #[cfg(feature = "code")]
        "hifz_link_code" => {
            state
                .client
                .post(format!("{}/api/v1/code/link", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }

        #[cfg(feature = "code")]
        "hifz_link_symbol" => {
            state
                .client
                .post(format!("{}/api/v1/code/link/symbol", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }

        #[cfg(feature = "code")]
        "hifz_code_gc" => {
            state
                .client
                .post(format!("{}/api/v1/code/gc", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }

        // --- atlas corpus-graph tools (feature-gated, proxied to REST) ---
        #[cfg(feature = "atlas")]
        "atlas_ingest" => {
            state
                .client
                .post(format!("{}/api/v1/atlas/ingest", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }
        #[cfg(feature = "atlas")]
        "atlas_code" => {
            state
                .client
                .post(format!("{}/api/v1/atlas/code", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }
        #[cfg(feature = "atlas")]
        "atlas_extract" => {
            state
                .client
                .post(format!("{}/api/v1/atlas/extract", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }
        #[cfg(feature = "atlas")]
        "atlas_cluster" => {
            state
                .client
                .post(format!("{}/api/v1/atlas/cluster", state.base_url))
                .json(&args)
                .send_decode()
                .await?
        }
        #[cfg(feature = "atlas")]
        "atlas_insights" => {
            state
                .client
                .get(format!("{}/api/v1/atlas/insights", state.base_url))
                .send_decode()
                .await?
        }
        #[cfg(feature = "atlas")]
        "atlas_query" => {
            let q = args.get("q").and_then(|v| v.as_str()).unwrap_or("");
            state
                .client
                .get(format!("{}/api/v1/atlas/query", state.base_url))
                .query(&[("q", q)])
                .send_decode()
                .await?
        }

        "hifz_usage" => {
            let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("");
            match mode {
                "session" => {
                    let sid = args
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    state
                        .client
                        .get(format!(
                            "{}/api/v1/agent/usage/session/{sid}",
                            state.base_url
                        ))
                        .send_decode()
                        .await?
                }
                "project" => {
                    let project = args.get("project").and_then(|v| v.as_str()).unwrap_or("");
                    let mut url =
                        format!("{}/api/v1/agent/usage/project/{project}", state.base_url);
                    let mut qs = Vec::new();
                    if let Some(from) = args.get("from").and_then(|v| v.as_str()) {
                        qs.push(format!("from={from}"));
                    }
                    if let Some(to) = args.get("to").and_then(|v| v.as_str()) {
                        qs.push(format!("to={to}"));
                    }
                    if let Some(model) = args.get("model").and_then(|v| v.as_str()) {
                        qs.push(format!("model={model}"));
                    }
                    if !qs.is_empty() {
                        url.push('?');
                        url.push_str(&qs.join("&"));
                    }
                    state.client.get(url).send_decode().await?
                }
                other => {
                    return Err(crate::mcp::http::ProxyError::BadArgs(format!(
                        "hifz_usage: unknown mode '{other}' (expected 'session' or 'project')"
                    ))
                    .into());
                }
            }
        }

        _ => {
            return Err(anyhow::anyhow!("Unknown tool: {name}"));
        }
    };

    Ok(serde_json::json!({ "content": mcp_content(&result)? }))
}

/// Build the MCP `content` blocks. Mutation tools (`hifz_save`/`hifz_evolve`/
/// `hifz_delete`) carry a `delta`; `hifz_view` returns a `MemoryView`. For
/// those, the first block is the human-readable rendered diff (what the
/// agent reads); the structured JSON always follows so nothing is lost.
fn mcp_content(result: &serde_json::Value) -> Result<Vec<serde_json::Value>> {
    let opts = memdiff::sink_text::TextOpts { colour: false };
    let mut blocks = Vec::new();

    if let Some(d) = result.get("delta") {
        if let Ok(delta) = serde_json::from_value::<memdiff::MemoryDelta>(d.clone())
            && !delta.lines.is_empty()
        {
            blocks.push(serde_json::json!({
                "type": "text",
                "text": memdiff::sink_text::render(&delta, &opts),
            }));
        }
    } else if result.get("header").is_some()
        && result.get("rows").is_some()
        && let Ok(view) = serde_json::from_value::<memdiff::MemoryView>(result.clone())
    {
        blocks.push(serde_json::json!({
            "type": "text",
            "text": memdiff::sink_text::render_view(&view, &opts),
        }));
    }

    blocks.push(serde_json::json!({
        "type": "text",
        "text": serde_json::to_string_pretty(result)?,
    }));
    Ok(blocks)
}

/// Generic pre-dispatch required-argument validator.
///
/// Every name in the matched tool's `inputSchema.required` must be present in
/// `args` with the JSON type its schema declares. This replaces N hand-rolled
/// per-tool guards with one uniform, self-correcting `-32602` (it names the
/// offending field, the expected type, and the keys actually received so the
/// model can fix the call in one retry). Tools with no `required` array — or
/// an unknown `name`, which the dispatch's `_ =>` arm rejects — pass through.
fn validate_required(name: &str, args: &serde_json::Value) -> Result<()> {
    #[allow(unused_mut)]
    let mut defs = tool_defs();
    #[cfg(feature = "atlas")]
    defs.extend(atlas_tool_defs());

    let Some(def) = defs
        .iter()
        .find(|d| d.get("name").and_then(|v| v.as_str()) == Some(name))
    else {
        return Ok(());
    };
    let schema = def.get("inputSchema");
    let Some(required) = schema
        .and_then(|s| s.get("required"))
        .and_then(|r| r.as_array())
    else {
        return Ok(());
    };
    let props = schema.and_then(|s| s.get("properties"));

    for field in required.iter().filter_map(|f| f.as_str()) {
        let expected = props
            .and_then(|p| p.get(field))
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str());
        let got = args.get(field);
        let present = !matches!(got, None | Some(serde_json::Value::Null));
        let type_ok = match (got, expected) {
            (Some(v), Some("string")) => v.is_string(),
            (Some(v), Some("integer")) => v.is_i64() || v.is_u64(),
            (Some(v), Some("number")) => v.is_number(),
            (Some(v), Some("boolean")) => v.is_boolean(),
            (Some(v), Some("array")) => v.is_array(),
            (Some(v), Some("object")) => v.is_object(),
            _ => true, // unknown/undeclared type: presence is enough
        };
        if !present || !type_ok {
            let ty = expected.unwrap_or("value");
            let received: Vec<&str> = args
                .as_object()
                .map(|o| o.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let reason = if !present {
                format!("missing required argument '{field}' (expected {ty})")
            } else {
                format!("argument '{field}' must be {ty}")
            };
            return Err(crate::mcp::http::ProxyError::BadArgs(format!(
                "{name}: {reason}. Received keys: {received:?}"
            ))
            .into());
        }
    }
    Ok(())
}

fn tool_defs() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"name": "hifz_recall", "description": "Search past observations and memories with graph expansion (optionally project-scoped)", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "project": {"type": "string"}, "session_id": {"type": "string", "description": "Session ID for provenance tracking"}}, "required": ["query"]}}),
        serde_json::json!({
            "name": "hifz_save",
            "description": "Save a memory to long-term store; only `content` is required (title is derived from it if omitted).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "content": {"type": "string"},
                    "content_long": {"type": "string", "description": "Full markdown body for long-form categories."},
                    "project": {"type": "string"},
                    "category": {
                        "type": "string",
                        "enum": [
                            "lesson", "decision", "bug", "fix", "gotcha",
                            "convention", "failure_pattern",
                            "plan", "design", "code_review", "ship_report", "context_slice",
                            "observation", "note"
                        ],
                        "default": "note"
                    },
                    "keywords": {"type": "array", "items": {"type": "string"}},
                    "files": {"type": "array", "items": {"type": "string"}},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "closes_memory_id": {"type": "string", "description": "Memory id this closes (fix→bug)."},
                    "supersedes_memory_id": {"type": "string", "description": "Memory id this replaces."},
                    "session_id": {"type": "string"}
                },
                "required": ["content"]
            }
        }),
        serde_json::json!({"name": "hifz_search", "description": "Hybrid semantic + keyword search with RRF fusion and graph expansion (optionally project-scoped)", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "project": {"type": "string"}, "session_id": {"type": "string", "description": "Session ID for provenance tracking"}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_sessions", "description": "List recent sessions", "inputSchema": {"type": "object", "properties": {"limit": {"type": "integer", "default": 20}}}}),
        serde_json::json!({"name": "hifz_digest", "description": "Get project intelligence — top keywords, files, and stats", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_timeline", "description": "Chronological observations", "inputSchema": {"type": "object", "properties": {"session_id": {"type": "string"}, "limit": {"type": "integer", "default": 50}}}}),
        serde_json::json!({"name": "hifz_timeline_causal", "description": "Causal timeline: time-ordered provenance chain (plan -> implementing commits -> code) from a seed node, or the project's active plan + recent commits. Walks only causal/provenance edges, not flat events.", "inputSchema": {"type": "object", "properties": {"seed": {"type": "string", "description": "Seed node id (e.g. 'memory:abc'); omit to use the project's active plan + recent commits"}, "project": {"type": "string"}, "max_hops": {"type": "integer", "default": 3}, "limit": {"type": "integer", "default": 20}}}}),
        serde_json::json!({"name": "hifz_export", "description": "Export all memory data", "inputSchema": {"type": "object", "properties": {}}}),
        serde_json::json!({"name": "hifz_delete", "description": "Delete a memory by ID", "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}}),
        serde_json::json!({"name": "hifz_core_get", "description": "Read the always-on core memory block for a project (identity, goals, invariants, watchlist)", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_core_edit", "description": "Edit the always-on core memory block. field=identity|goals|invariants|watchlist, op=set|add|remove", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "field": {"type": "string", "enum": ["identity", "goals", "invariants", "watchlist"]}, "op": {"type": "string", "enum": ["set", "add", "remove"]}, "value": {"type": "string"}}, "required": ["project", "field", "op", "value"]}}),
        serde_json::json!({"name": "hifz_runs", "description": "Search past task-scoped runs (prompt + derived lesson) via hybrid BM25 fusion", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}, "limit": {"type": "integer", "default": 10}}, "required": ["query"]}}),
        serde_json::json!({"name": "hifz_commits", "description": "List recent git commits for a project. Use this to see repo history and continue from a specific point.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "branch": {"type": "string"}, "limit": {"type": "integer", "default": 10}, "sha": {"type": "string", "description": "Get a specific commit by SHA"}}}}),
        serde_json::json!({"name": "hifz_evolve", "description": "Run A-MEM Memory Evolution on a memory — LLM refines neighbour tags/context/links (requires HIFZ_LLM_EVOLVE=true and Ollama)", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string", "description": "RecordId like 'memory:xyz'"}}, "required": ["memory_id"]}}),
        serde_json::json!({"name": "hifz_view", "description": "Inspect a memory as a rendered view — its lineage (superseded rows), outgoing links, and evolution history.", "inputSchema": {"type": "object", "properties": {"id": {"type": "string", "description": "Memory id like 'memory:xyz' or the bare key"}}, "required": ["id"]}}),
        serde_json::json!({"name": "hifz_current_plan", "description": "Get the currently active plan for this project. Returns null if no active plan.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_plans", "description": "List plans for a project. Filter by status (active, completed, abandoned, all).", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "status": {"type": "string", "enum": ["active", "completed", "abandoned", "all"], "default": "all"}, "limit": {"type": "integer", "default": 10}}}}),
        serde_json::json!({"name": "hifz_complete_plan", "description": "Mark the current active plan as completed.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}}),
        serde_json::json!({"name": "hifz_activate_plan", "description": "Activate a plan for this session. Adds plan title to core memory goals and files to watchlist. The plan will then be injected into context.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "plan_id": {"type": "string", "description": "Optional. If omitted, activates the most recent active plan."}, "session_id": {"type": "string", "description": "Session ID for provenance tracking"}}}}),
        serde_json::json!({"name": "hifz_trace", "description": "Trace the knowledge graph from a starting node. Returns nodes and edges showing provenance, similarity, and causal relationships.", "inputSchema": {"type": "object", "properties": {"id": {"type": "string", "description": "Starting node ID (e.g. 'memory:abc', 'run:xyz')"}, "ids": {"type": "array", "items": {"type": "string"}, "description": "Multiple seed node IDs for convergence/divergence trace; takes precedence over `id`"}, "direction": {"type": "string", "enum": ["forward", "backward", "both"], "default": "both"}, "relations": {"type": "array", "items": {"type": "string"}, "description": "Filter to specific relation types"}, "max_hops": {"type": "integer", "default": 2}}, "required": ["id"]}}),
        // Phase 9: typed graph + project surface tools.
        serde_json::json!({"name": "hifz_neighbors", "description": "Walk the typed graph from a memory. Use this to find related memories by specific relation kinds: conceptual (related/broader/narrower), argumentative (supports/contradicts/elaborates), lifecycle (closes/supersedes), or co-occurrence (co_occurs_*).", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string", "description": "RecordId like 'memory:abc'"}, "relations": {"type": "array", "items": {"type": "string", "enum": ["co_occurs_files", "co_occurs_keywords", "co_occurs_embedding", "mentions", "generated_by", "informed_by", "derived_from", "attributed_to", "part_of", "follows", "broader", "narrower", "related", "same_as", "supports", "contradicts", "elaborates", "responds_to", "supersedes", "closes", "touches_file", "commits_for", "tests"]}, "description": "Filter to specific typed relations. Empty = all relations."}, "max_hops": {"type": "integer", "default": 1, "minimum": 1, "maximum": 4}}, "required": ["memory_id"]}}),
        serde_json::json!({"name": "hifz_backlinks", "description": "List incoming edges for a memory — every memory or observation that references it. Optional relation filter.", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string"}, "relation": {"type": "string", "description": "Optional: limit to one typed relation"}}, "required": ["memory_id"]}}),
        serde_json::json!({"name": "hifz_warmup", "description": "Build the project-scoped warmup digest — latest plan, recent decisions, conventions, open bugs, gotchas, failure patterns, recent lessons. Inject the `top` field as system context at session start to give the agent a 'here's where you are' snapshot.", "inputSchema": {"type": "object", "properties": {"session_id": {"type": "string"}, "project": {"type": "string", "description": "Defaults to the session's project."}, "top_n": {"type": "integer", "default": 15}}, "required": ["session_id"]}}),
        serde_json::json!({"name": "hifz_project_accumulators", "description": "Get the project's cross-cutting rollup: latest plan, decisions, conventions, open bugs, gotchas, failure patterns, recent lessons. Like hifz_warmup but project-only (no session scope).", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}}),
        serde_json::json!({"name": "hifz_project_digest", "description": "Chronological digest of recent activity for a project, grouped by typed category. Powers a 'what happened in the last N days' view.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "days": {"type": "integer", "default": 30, "minimum": 1}}, "required": ["project"]}}),
        serde_json::json!({
            "name": "hifz_usage",
            "description": "LLM token usage scoped to a session or a project. Generic over any agent that posts to /api/v1/agent/usage — no Claude-specific concepts. mode='session' returns per-call array + totals + session patterns; mode='project' returns daily/model/top-prompts/top-sessions + project patterns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode":       {"type": "string", "enum": ["session", "project"]},
                    "session_id": {"type": "string", "description": "Required when mode=session."},
                    "project":    {"type": "string", "description": "Required when mode=project."},
                    "from":       {"type": "string", "description": "YYYY-MM-DD lower bound (project mode)."},
                    "to":         {"type": "string", "description": "YYYY-MM-DD upper bound (project mode)."},
                    "model":      {"type": "string", "description": "Filter by model id (project mode)."}
                },
                "required": ["mode"]
            }
        }),
        // Code dimension (M2+) — gated by `code` Cargo feature.
        #[cfg(feature = "code")]
        serde_json::json!({"name": "hifz_code_index", "description": "Index a repo: chunk + embed source files for semantic code search (idempotent).", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "root": {"type": "string", "description": "Absolute path to repo root"}, "follow_symlinks": {"type": "boolean", "default": false}, "max_file_bytes": {"type": "integer", "default": 2097152}}, "required": ["project", "root"]}}),
        #[cfg(feature = "code")]
        serde_json::json!({"name": "hifz_code_search", "description": "Hybrid (vector + BM25) search over indexed code chunks. Returns paths with line ranges and snippets.", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}, "language": {"type": "string", "description": "Filter by language: rust|python|typescript|javascript|go|java|c|cpp"}, "path": {"type": "string", "description": "Substring filter against code_chunk.path"}, "limit": {"type": "integer", "default": 10}, "group_by_file": {"type": "boolean", "default": false}}, "required": ["query"]}}),
        #[cfg(feature = "code")]
        serde_json::json!({"name": "hifz_link_code", "description": "Link a memory to a precise code location. Creates Memory --references--> CodeChunk edges for every chunk overlapping the line range.", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string"}, "file": {"type": "string", "description": "Repo-relative POSIX path"}, "start_line": {"type": "integer", "minimum": 1}, "end_line": {"type": "integer", "minimum": 1, "description": "Defaults to start_line"}, "project": {"type": "string"}, "reason": {"type": "string"}}, "required": ["memory_id", "file", "start_line"]}}),
        #[cfg(feature = "code")]
        serde_json::json!({"name": "hifz_link_symbol", "description": "Link a memory to a named code symbol (function/struct/class/...). Creates Memory --references_symbol--> CodeSymbol edge(s).", "inputSchema": {"type": "object", "properties": {"memory_id": {"type": "string"}, "name": {"type": "string", "description": "Symbol name (or qualified module::name)"}, "kind": {"type": "string", "description": "Optional kind filter: function|struct|enum|trait|method|class|interface|const|module|namespace|type|macro"}, "file": {"type": "string", "description": "Optional path to disambiguate"}, "project": {"type": "string"}, "reason": {"type": "string"}}, "required": ["memory_id", "name"]}}),
        #[cfg(feature = "code")]
        serde_json::json!({"name": "hifz_code_gc", "description": "Reconcile code-index against the filesystem: drop chunks/symbols/edges for deleted files; optionally decay cold chunks. Run after large refactors or when stale entries linger.", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}, "root": {"type": "string"}, "dry_run": {"type": "boolean", "default": false}, "force_decay": {"type": "boolean", "default": false}}, "required": ["project", "root"]}}),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_msg(name: &str, args: serde_json::Value) -> String {
        validate_required(name, &args)
            .expect_err("expected validation error")
            .to_string()
    }

    #[test]
    fn hifz_save_title_no_longer_required() {
        // content only, no title — must pass now that title is derived.
        validate_required("hifz_save", &serde_json::json!({"content": "X happened"}))
            .expect("content-only hifz_save should validate");
    }

    #[test]
    fn hifz_save_missing_content_is_self_correcting_32602() {
        let m = err_msg("hifz_save", serde_json::json!({"title": "just a title"}));
        assert!(m.contains("hifz_save"), "{m}");
        assert!(m.contains("'content'"), "{m}");
        assert!(m.contains("missing required"), "{m}");
        // names the keys actually received so the model can self-correct
        assert!(m.contains("title"), "{m}");
    }

    #[test]
    fn hifz_save_content_wrong_type_rejected() {
        let m = err_msg("hifz_save", serde_json::json!({"content": 42}));
        assert!(
            m.contains("'content'") && m.contains("must be string"),
            "{m}"
        );
    }

    #[test]
    fn generic_layer_covers_other_multi_required_tools() {
        // hifz_core_edit requires project, field, op, value — `op` omitted.
        let m = err_msg(
            "hifz_core_edit",
            serde_json::json!({"project": "p", "field": "goals", "value": "v"}),
        );
        assert!(m.contains("hifz_core_edit") && m.contains("'op'"), "{m}");

        validate_required(
            "hifz_core_edit",
            &serde_json::json!({"project": "p", "field": "goals", "op": "add", "value": "v"}),
        )
        .expect("fully-specified hifz_core_edit should validate");
    }

    #[test]
    fn unknown_tool_and_no_required_pass_through() {
        validate_required("not_a_tool", &serde_json::json!({})).expect("unknown name passes");
        // hifz_sessions has no `required` array.
        validate_required("hifz_sessions", &serde_json::json!({})).expect("no-required passes");
    }

    #[test]
    fn list_tools_advertises_only_core() {
        let v = list_tools().expect("list_tools");
        let names: Vec<String> = v["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.is_empty());
        for n in &names {
            assert!(
                CORE_TOOLS.contains(&n.as_str()),
                "advertised non-core tool: {n}"
            );
        }
        assert!(
            !names.iter().any(|n| n.starts_with("atlas_")),
            "atlas tools must not be advertised"
        );
        // feature-independent core members are all present
        for must in [
            "hifz_recall",
            "hifz_save",
            "hifz_search",
            "hifz_delete",
            "hifz_sessions",
            "hifz_current_plan",
            "hifz_plans",
            "hifz_activate_plan",
            "hifz_complete_plan",
        ] {
            assert!(names.iter().any(|n| n == must), "missing core tool: {must}");
        }
    }

    #[test]
    fn hidden_tools_still_defined_for_dispatch_and_validate() {
        // Hidden ≠ disabled: not advertised, but still in tool_defs() so
        // call_tool + validate_required keep working (also reachable via REST).
        let defs = tool_defs();
        let all: Vec<&str> = defs.iter().filter_map(|t| t["name"].as_str()).collect();
        for hidden in [
            "hifz_timeline",
            "hifz_usage",
            "hifz_core_edit",
            "hifz_trace",
        ] {
            assert!(all.contains(&hidden), "hidden tool dropped: {hidden}");
            assert!(!CORE_TOOLS.contains(&hidden), "{hidden} unexpectedly core");
        }
        let m = err_msg("hifz_trace", serde_json::json!({}));
        assert!(m.contains("hifz_trace") && m.contains("'id'"), "{m}");
    }
}
