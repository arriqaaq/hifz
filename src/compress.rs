use crate::models::HookPayload;
use crate::ollama::OllamaClient;
use crate::prompts;

/// Result of synthetic or LLM compression.
pub struct CompressResult {
    pub obs_type: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub facts: Vec<String>,
    pub narrative: String,
    pub concepts: Vec<String>,
    pub files: Vec<String>,
    pub importance: i64,
    pub confidence: Option<f64>,
}

/// Synthetic compression: extract structured data from raw hook payload without LLM.
/// This is the default path (HIFZ_AUTO_COMPRESS=false).
pub fn compress_synthetic(payload: &HookPayload) -> CompressResult {
    let data = &payload.data;
    let tool_name = data
        .get("tool_name")
        .or_else(|| data.get("toolName"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let obs_type = infer_obs_type(tool_name, &payload.hook_type);
    let title = build_title(tool_name, data);
    let facts = extract_facts(data);
    let files = extract_files(data);
    let concepts = extract_concepts(&files, tool_name);
    let narrative = build_narrative(tool_name, &payload.hook_type, data);
    let importance = infer_importance(&payload.hook_type, tool_name);

    CompressResult {
        obs_type,
        title,
        subtitle: None,
        facts,
        narrative,
        concepts,
        files,
        importance,
        confidence: Some(0.5), // synthetic = moderate confidence
    }
}

/// LLM-powered compression via Ollama (optional, when HIFZ_AUTO_COMPRESS=true).
pub async fn compress_llm(
    payload: &HookPayload,
    ollama: &OllamaClient,
) -> anyhow::Result<CompressResult> {
    let user_prompt = serde_json::to_string_pretty(&payload.data)?;
    let response = ollama
        .complete(prompts::COMPRESSION_SYSTEM, &user_prompt)
        .await?;

    // Parse XML response
    parse_compression_xml(&response)
}

fn parse_compression_xml(xml: &str) -> anyhow::Result<CompressResult> {
    let extract = |tag: &str| -> String {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        xml.find(&open)
            .and_then(|start| {
                let content_start = start + open.len();
                xml[content_start..]
                    .find(&close)
                    .map(|end| xml[content_start..content_start + end].trim().to_string())
            })
            .unwrap_or_default()
    };

    let extract_list = |tag: &str, item_tag: &str| -> Vec<String> {
        let section = extract(tag);
        let item_open = format!("<{item_tag}>");
        let item_close = format!("</{item_tag}>");
        let mut items = Vec::new();
        let mut search_from = 0;
        while let Some(start) = section[search_from..].find(&item_open) {
            let content_start = search_from + start + item_open.len();
            if let Some(end) = section[content_start..].find(&item_close) {
                let item = section[content_start..content_start + end]
                    .trim()
                    .to_string();
                if !item.is_empty() {
                    items.push(item);
                }
                search_from = content_start + end + item_close.len();
            } else {
                break;
            }
        }
        items
    };

    Ok(CompressResult {
        obs_type: extract("type"),
        title: extract("title"),
        subtitle: {
            let s = extract("subtitle");
            if s.is_empty() { None } else { Some(s) }
        },
        facts: extract_list("facts", "fact"),
        narrative: extract("narrative"),
        concepts: extract_list("concepts", "concept"),
        files: extract_list("files", "file"),
        importance: extract("importance").parse().unwrap_or(5),
        confidence: Some(0.8),
    })
}

fn infer_obs_type(tool_name: &str, hook_type: &str) -> String {
    match tool_name {
        "Read" => "file_read",
        "Write" => "file_write",
        "Edit" => "file_edit",
        "Bash" => "command_run",
        "Grep" | "Glob" => "search",
        "WebFetch" | "WebSearch" => "web_fetch",
        _ => match hook_type {
            "post_tool_failure" => "error",
            "prompt_submit" => "conversation",
            "subagent_start" | "subagent_stop" => "subagent",
            "notification" => "notification",
            "task_completed" => "task",
            "post_compact" => "compaction_summary",
            _ => "other",
        },
    }
    .to_string()
}

fn build_title(tool_name: &str, data: &serde_json::Value) -> String {
    let file_path = data
        .get("tool_input")
        .or_else(|| data.get("toolInput"))
        .and_then(|v| v.get("file_path").or_else(|| v.get("filePath")))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !file_path.is_empty() {
        format!("{tool_name}: {file_path}")
    } else {
        format!("{tool_name} call")
    }
}

fn extract_facts(data: &serde_json::Value) -> Vec<String> {
    let mut facts = Vec::new();
    if let Some(input) = data.get("tool_input").or_else(|| data.get("toolInput")) {
        if let Some(obj) = input.as_object() {
            for (key, val) in obj {
                let val_str = match val {
                    serde_json::Value::String(s) => {
                        if s.len() > 200 {
                            format!("{}...", &s[..200])
                        } else {
                            s.clone()
                        }
                    }
                    _ => {
                        let s = val.to_string();
                        if s.len() > 200 {
                            format!("{}...", &s[..200])
                        } else {
                            s
                        }
                    }
                };
                facts.push(format!("{key}: {val_str}"));
            }
        }
    }
    facts
}

fn extract_files(data: &serde_json::Value) -> Vec<String> {
    let mut files = Vec::new();
    let input = data.get("tool_input").or_else(|| data.get("toolInput"));
    if let Some(input) = input {
        for key in &["file_path", "filePath", "path", "file"] {
            if let Some(val) = input.get(*key).and_then(|v| v.as_str()) {
                files.push(val.to_string());
            }
        }
    }
    files
}

fn extract_concepts(files: &[String], tool_name: &str) -> Vec<String> {
    let mut concepts = Vec::new();
    for f in files {
        // Extract directory components as concepts
        if let Some(parent) = std::path::Path::new(f).parent() {
            for comp in parent.components() {
                let s = comp.as_os_str().to_string_lossy().to_string();
                if s.len() > 2 && s != "src" && s != "." && !concepts.contains(&s) {
                    concepts.push(s);
                }
            }
        }
        // File extension as concept
        if let Some(ext) = std::path::Path::new(f).extension() {
            let ext_str = ext.to_string_lossy().to_string();
            if !concepts.contains(&ext_str) {
                concepts.push(ext_str);
            }
        }
    }
    if !concepts.contains(&tool_name.to_lowercase()) {
        concepts.push(tool_name.to_lowercase());
    }
    concepts
}

fn build_narrative(tool_name: &str, hook_type: &str, data: &serde_json::Value) -> String {
    let file_path = data
        .get("tool_input")
        .or_else(|| data.get("toolInput"))
        .and_then(|v| v.get("file_path").or_else(|| v.get("filePath")))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match hook_type {
        "post_tool_use" => {
            if file_path.is_empty() {
                format!("Used {tool_name} tool.")
            } else {
                format!("Used {tool_name} on {file_path}.")
            }
        }
        "post_tool_failure" => format!("{tool_name} failed."),
        "session_start" => "Session started.".to_string(),
        "session_end" => "Session ended.".to_string(),
        "prompt_submit" => "User submitted a prompt.".to_string(),
        _ => format!("Hook {hook_type} fired for {tool_name}."),
    }
}

fn infer_importance(hook_type: &str, tool_name: &str) -> i64 {
    match hook_type {
        "post_tool_failure" => 7,
        "session_start" | "session_end" => 3,
        "prompt_submit" => 4,
        _ => match tool_name {
            "Write" => 6,
            "Edit" => 6,
            "Bash" => 5,
            "Read" | "Glob" | "Grep" => 2,
            _ => 3,
        },
    }
}
