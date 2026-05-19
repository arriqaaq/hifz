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
    pub keywords: Vec<String>,
    pub files: Vec<String>,
    pub importance: i64,
    pub confidence: Option<f64>,
    /// Adapter-supplied structured signal, passed through verbatim to the
    /// observation row (schema field `observation.metadata`). For
    /// `commit_made` this carries the git polarity signal consumed by
    /// grounding.
    pub metadata: Option<serde_json::Value>,
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

    let obs_type = payload
        .obs_type
        .clone()
        .unwrap_or_else(|| infer_obs_type(tool_name, &payload.hook_type));
    let title = if payload.hook_type == "prompt_submit" {
        let prompt = data.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        if prompt.len() > 60 {
            format!("Prompt: {}…", crate::truncate_at_char_boundary(prompt, 60))
        } else if !prompt.is_empty() {
            format!("Prompt: {prompt}")
        } else {
            "Prompt".to_string()
        }
    } else if payload.hook_type == "post_compact" {
        // post_compact has no tool_name/file_path; the actual signal is in
        // `data.summary` (+ `data.trigger`). Without a dedicated arm here
        // and in `build_narrative`, the row lands as "unknown call" /
        // "Hook post_compact fired for unknown." and the summary is dropped.
        let trigger = data.get("trigger").and_then(|v| v.as_str()).unwrap_or("");
        if trigger.is_empty() {
            "Compaction summary".to_string()
        } else {
            format!("Compaction summary ({trigger})")
        }
    } else {
        build_title(tool_name, data)
    };
    let facts = extract_facts(data);
    // A `commit_made` payload carries a git command in `data` (no file
    // paths), so the changed-file set only arrives via adapter `metadata`.
    // Without this, `compressed.files` is empty and grounding is inert.
    let files = if obs_type == "commit_made" {
        payload
            .metadata
            .as_ref()
            .and_then(|m| m.get("files"))
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| extract_files(data))
    } else {
        extract_files(data)
    };
    let keywords = extract_keywords(&files, tool_name);
    let narrative = build_narrative(tool_name, &payload.hook_type, data);
    let importance = infer_importance(&payload.hook_type, tool_name);

    CompressResult {
        obs_type,
        title,
        subtitle: None,
        facts,
        narrative,
        keywords,
        files,
        importance,
        confidence: Some(0.5), // synthetic = moderate confidence
        metadata: payload.metadata.clone(),
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

    let mut result = parse_compression_xml(&response)?;
    // Adapter-supplied obs_type overrides LLM inference
    if let Some(ref ot) = payload.obs_type {
        result.obs_type = ot.clone();
    }
    // Carry adapter metadata through the LLM path too (the model never sees
    // or produces it).
    result.metadata = payload.metadata.clone();
    Ok(result)
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
        keywords: extract_list("keywords", "keyword"),
        files: extract_list("files", "file"),
        importance: extract("importance").parse().unwrap_or(5),
        confidence: Some(0.8),
        metadata: None,
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
    if tool_name == "Bash" || tool_name == "Shell" {
        let command = data
            .get("tool_input")
            .or_else(|| data.get("toolInput"))
            .and_then(|v| v.get("command"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !command.is_empty() {
            // Extract just the first meaningful line/command, skip comments
            let first_cmd = command
                .lines()
                .map(|l| l.trim())
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .unwrap_or(command);
            // Truncate at first pipe or 80 chars (UTF-8 safe).
            let short = match first_cmd.find('|') {
                Some(pos) if pos < 80 => &first_cmd[..pos],
                _ if first_cmd.len() > 80 => crate::truncate_at_char_boundary(first_cmd, 80),
                _ => first_cmd,
            };
            return format!("{tool_name}: {}", short.trim());
        }
    }

    let file_path = data
        .get("tool_input")
        .or_else(|| data.get("toolInput"))
        .and_then(|v| v.get("file_path").or_else(|| v.get("filePath")))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !file_path.is_empty() {
        let basename = file_path.rsplit('/').next().unwrap_or(file_path);
        format!("{tool_name}: {basename}")
    } else {
        format!("{tool_name} call")
    }
}

fn extract_facts(data: &serde_json::Value) -> Vec<String> {
    let mut facts = Vec::new();
    if let Some(input) = data.get("tool_input").or_else(|| data.get("toolInput"))
        && let Some(obj) = input.as_object()
    {
        for (key, val) in obj {
            let val_str = match val {
                serde_json::Value::String(s) => {
                    if s.len() > 200 {
                        format!("{}...", crate::truncate_at_char_boundary(s, 200))
                    } else {
                        s.clone()
                    }
                }
                _ => {
                    let s = val.to_string();
                    if s.len() > 200 {
                        format!("{}...", crate::truncate_at_char_boundary(&s, 200))
                    } else {
                        s
                    }
                }
            };
            facts.push(format!("{key}: {val_str}"));
        }
    }

    // For Bash/Shell: include tool_output as a fact
    let tool_name = data
        .get("tool_name")
        .or_else(|| data.get("toolName"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if (tool_name == "Bash" || tool_name == "Shell")
        && let Some(output) = data
            .get("tool_output")
            .or_else(|| data.get("toolOutput"))
            .and_then(|v| v.as_str())
        && !output.is_empty()
    {
        let truncated = if output.len() > 300 {
            format!("{}...", crate::truncate_at_char_boundary(output, 300))
        } else {
            output.to_string()
        };
        facts.push(format!("output: {truncated}"));
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

const NOISE_DIRS: &[&str] = &[
    "/",
    "Users",
    "home",
    "root",
    "var",
    "tmp",
    "opt",
    "usr",
    "workspace",
    "projects",
    "repos",
    "code",
    "dev",
    "Documents",
    "Desktop",
    "Downloads",
];

fn extract_keywords(files: &[String], tool_name: &str) -> Vec<String> {
    let mut kws = Vec::new();
    for f in files {
        if let Some(parent) = std::path::Path::new(f).parent() {
            for comp in parent.components() {
                let s = comp.as_os_str().to_string_lossy().to_string();
                if s.len() > 2
                    && s != "src"
                    && s != "."
                    && !NOISE_DIRS.contains(&s.as_str())
                    && !kws.contains(&s)
                {
                    kws.push(s);
                }
            }
        }
        if let Some(ext) = std::path::Path::new(f).extension() {
            let ext_str = ext.to_string_lossy().to_string();
            if !kws.contains(&ext_str) {
                kws.push(ext_str);
            }
        }
    }
    if !kws.contains(&tool_name.to_lowercase()) {
        kws.push(tool_name.to_lowercase());
    }
    kws
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
            if tool_name == "Bash" || tool_name == "Shell" {
                // Show just the output summary, not the command (title already has it)
                let output = data
                    .get("tool_output")
                    .or_else(|| data.get("toolOutput"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if output.is_empty() {
                    return "(no output)".to_string();
                }
                let last_lines: String = output
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ");
                if last_lines.len() > 200 {
                    format!("{}…", crate::truncate_at_char_boundary(&last_lines, 200))
                } else {
                    last_lines
                }
            } else if file_path.is_empty() {
                format!("Used {tool_name} tool.")
            } else {
                format!("Used {tool_name} on {file_path}.")
            }
        }
        "post_tool_failure" => format!("{tool_name} failed."),
        "session_start" => "Session started.".to_string(),
        "session_end" => "Session ended.".to_string(),
        "prompt_submit" => {
            let prompt = data.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            if prompt.is_empty() {
                "User submitted a prompt.".to_string()
            } else if prompt.len() > 120 {
                format!("{}…", crate::truncate_at_char_boundary(prompt, 120))
            } else {
                prompt.to_string()
            }
        }
        "post_compact" => {
            // The compaction summary itself is the narrative. Without this
            // arm the catch-all writes "Hook post_compact fired for unknown."
            // and the actual summary in `data.summary` is silently dropped.
            //
            // No length cap: the narrative field is `TYPE string` (unbounded)
            // and a compaction summary is the irreducible distillation of a
            // whole conversation window — truncating defeats the point.
            // The embedder (fastembed MiniLM, 256-token window) truncates
            // internally when computing the vector; the *stored* text stays
            // complete and re-renders fully in the UI on recall.
            let summary = data.get("summary").and_then(|v| v.as_str()).unwrap_or("");
            if summary.is_empty() {
                "Conversation was compacted (no summary provided).".to_string()
            } else {
                summary.to_string()
            }
        }
        _ => format!("Hook {hook_type} fired for {tool_name}."),
    }
}

fn infer_importance(hook_type: &str, tool_name: &str) -> i64 {
    match hook_type {
        "post_tool_failure" => 7,
        "session_start" | "session_end" => 3,
        "prompt_submit" => 4,
        // A compaction summary distils a whole conversation window — it is
        // strictly higher-signal than a single Read/Edit and deserves to
        // surface in recall ahead of those.
        "post_compact" => 6,
        _ => match tool_name {
            "Write" => 6,
            "Edit" => 6,
            "Bash" => 5,
            "Read" | "Glob" | "Grep" => 2,
            _ => 3,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(hook_type: &str, data: serde_json::Value) -> HookPayload {
        HookPayload {
            hook_type: hook_type.to_string(),
            session_id: "s".into(),
            project: "p".into(),
            cwd: "/tmp".into(),
            timestamp: "2026-01-01T00:00:00Z".into(),
            source: None,
            obs_type: None,
            parent_obs_id: None,
            data,
            metadata: None,
        }
    }

    /// Regression fence: post_compact must not produce
    /// "unknown call" / "Hook post_compact fired for unknown." — the
    /// failure mode that silently dropped every compaction summary.
    #[test]
    fn post_compact_compresses_summary_into_title_and_narrative() {
        let summary = "Refactored atlas_edge to denormalize project; tests 25/25 green.";
        let p = payload(
            "post_compact",
            json!({ "trigger": "manual", "custom_instructions": "", "summary": summary }),
        );
        let r = compress_synthetic(&p);

        assert_eq!(r.obs_type, "compaction_summary");
        assert!(
            r.title.starts_with("Compaction summary"),
            "title was {:?}",
            r.title
        );
        assert!(r.title.contains("manual"), "title was {:?}", r.title);
        assert_eq!(r.narrative, summary);
        // Stays above per-tool noise (Read=2) so compactions surface in recall.
        assert!(r.importance >= 5, "importance was {}", r.importance);

        // Anti-pattern: the broken placeholders must never appear.
        assert_ne!(r.title, "unknown call");
        assert!(
            !r.narrative.starts_with("Hook post_compact fired"),
            "narrative leaked the catch-all: {:?}",
            r.narrative
        );
    }

    /// Empty-summary edge case: a malformed compaction event must still
    /// yield a usable placeholder, not "unknown call".
    #[test]
    fn post_compact_with_missing_summary_still_has_usable_title_and_narrative() {
        let p = payload("post_compact", json!({ "trigger": "" }));
        let r = compress_synthetic(&p);
        assert_eq!(r.obs_type, "compaction_summary");
        assert_eq!(r.title, "Compaction summary");
        assert!(r.narrative.contains("compacted"), "{:?}", r.narrative);
        assert_ne!(r.title, "unknown call");
    }

    /// A real Claude Code compaction summary is multi-kilobyte. The narrative
    /// field is `TYPE string` (unbounded) and the summary is the irreducible
    /// signal — it must be stored byte-for-byte, not truncated. Embedding
    /// happens downstream and may truncate the *vector* input, but the
    /// stored text stays whole so recall renders the full summary.
    #[test]
    fn post_compact_stores_full_multi_kilobyte_summary_verbatim() {
        let big = "x".repeat(8192) + "<<END_MARKER>>";
        let p = payload(
            "post_compact",
            json!({ "trigger": "auto", "summary": big.clone() }),
        );
        let r = compress_synthetic(&p);
        assert_eq!(r.narrative.len(), big.len(), "narrative was truncated");
        assert!(
            r.narrative.ends_with("<<END_MARKER>>"),
            "tail of summary was dropped"
        );
        assert_eq!(r.narrative, big);
    }
}
