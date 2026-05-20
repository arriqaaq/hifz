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
        // No length cap. Scan-line shortening is a render concern (CSS
        // `text-overflow: ellipsis`); the writer must not destroy bytes
        // just so the timeline UI stays narrow.
        let prompt = data.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        if prompt.is_empty() {
            "Prompt".to_string()
        } else {
            format!("Prompt: {prompt}")
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
            // No truncation. Previous logic stripped to "first meaningful
            // line" then capped at 80 chars / first pipe — both lossy:
            // a piped command lost everything after the pipe; a multi-line
            // command was reduced to its `cd …` prologue. The full command
            // is structurally one unit and should live in the title; any
            // visual shortening is a render concern.
            return format!("{tool_name}: {}", command.trim());
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
            // No cap: `facts` is unbounded `array<string>` in the schema,
            // `payload.data` is not persisted (observe.rs:190-208), so this
            // is the only stored copy of e.g. Write.content / Edit.new_string.
            // Truncating here meant losing the file content forever; any
            // scan-line shortening belongs in the renderer, not the writer.
            let val_str = match val {
                serde_json::Value::String(s) => s.clone(),
                _ => val.to_string(),
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
        // Bash stdout: only stored copy (`data.tool_output` is not persisted —
        // observe.rs:190-208). No cap: truncating cargo/test/lint output
        // throws away exactly the lines a future recall would want. The
        // schema is unbounded; render-side truncation is the right place
        // to keep the timeline readable.
        facts.push(format!("output: {output}"));
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
                // Tail of output (last 3 lines) — the high-signal slice
                // for errors and result summaries. No length cap: lines are
                // self-bounded to 3, and a pathological single-line JSON
                // dump should still be persisted whole (the renderer can
                // collapse it). Anything we drop here is irretrievable.
                output
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ")
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
            // The prompt is the irreducible user signal — the whole point
            // of saving the observation. Title (`compress_synthetic` at
            // line 37-46) already produces the "Prompt: …" scan-line capped
            // at 60 chars; the narrative is where the *full* prompt belongs.
            // `narrative` is `TYPE string` (unbounded); the embedder
            // (fastembed MiniLM, 256-token window) truncates internally for
            // the vector, so there is no downstream cost to storing whole.
            //
            // (Same class of bug as the post_compact arm above, which
            // previously dropped the entire compaction summary.)
            let prompt = data.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            if prompt.is_empty() {
                "User submitted a prompt.".to_string()
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

    /// Regression fence: a multi-kilobyte prompt must land in both `title`
    /// and `narrative` byte-for-byte. No length caps anywhere on the write
    /// path — the prompt is the user's irreducible record and any scan-line
    /// shortening is a render concern (CSS `text-overflow: ellipsis`).
    #[test]
    fn prompt_submit_stores_full_prompt_unchanged() {
        let big = "x".repeat(8192) + "<<END>>";
        let p = payload("prompt_submit", json!({ "prompt": big.clone() }));
        let r = compress_synthetic(&p);
        // Narrative: full prompt verbatim.
        assert_eq!(r.narrative.len(), big.len(), "narrative was truncated");
        assert!(r.narrative.ends_with("<<END>>"));
        assert_eq!(r.narrative, big);
        // Title: "Prompt: " + full prompt verbatim. Confirms the renderer
        // sees the whole string; visual trim is downstream.
        assert_eq!(r.title, format!("Prompt: {big}"));
        assert!(r.title.ends_with("<<END>>"));
    }

    /// Bash output fact: stdout of any size must round-trip verbatim — no
    /// 300/4096 cap, no truncation marker. The fact is the only stored copy
    /// of `data.tool_output`; clipping it dropped cargo/test traces.
    #[test]
    fn bash_output_fact_stores_full_stdout_verbatim() {
        let out = "line\n".repeat(2000) + "<<TAIL>>"; // ~10 KB
        let p = payload(
            "post_tool_use",
            json!({
                "tool_name": "Bash",
                "tool_input": { "command": "echo hi" },
                "tool_output": out.clone(),
            }),
        );
        let r = compress_synthetic(&p);
        let output_fact = r
            .facts
            .iter()
            .find(|f| f.starts_with("output: "))
            .expect("missing output fact");
        assert_eq!(output_fact, &format!("output: {out}"));
        assert!(
            output_fact.ends_with("<<TAIL>>"),
            "tail of stdout was dropped"
        );
        assert!(
            !output_fact.contains("..."),
            "fact carries the old truncation marker: {output_fact}"
        );
    }

    /// Bash command in title: full command preserved (no 80-char cap, no
    /// pipe-truncation). Previously a piped command lost everything after
    /// the first `|`; a multi-line command was reduced to its `cd …` prologue.
    #[test]
    fn bash_command_title_preserves_full_pipeline() {
        let cmd = "cd /repo && cargo test --release \
                   --features atlas | tee /tmp/out | grep -E 'test result|FAILED'";
        let p = payload(
            "post_tool_use",
            json!({
                "tool_name": "Bash",
                "tool_input": { "command": cmd },
                "tool_output": "",
            }),
        );
        let r = compress_synthetic(&p);
        assert!(
            r.title.contains("tee /tmp/out"),
            "title lost content after pipe: {}",
            r.title
        );
        assert!(
            r.title.contains("FAILED"),
            "title lost content after second pipe: {}",
            r.title
        );
        assert!(
            !r.title.contains("…"),
            "title still carries the old ellipsis: {}",
            r.title
        );
    }

    /// Tool_input value preserved unchanged: a Write of a multi-kilobyte
    /// `content` field used to be clipped at 200 chars, dropping nearly the
    /// entire file from the stored fact.
    #[test]
    fn tool_input_value_fact_stores_full_value_verbatim() {
        let body = "x".repeat(5000) + "<<EOF>>";
        let p = payload(
            "post_tool_use",
            json!({
                "tool_name": "Write",
                "tool_input": { "file_path": "/tmp/big.md", "content": body.clone() },
            }),
        );
        let r = compress_synthetic(&p);
        let content_fact = r
            .facts
            .iter()
            .find(|f| f.starts_with("content: "))
            .expect("missing content fact");
        assert!(content_fact.ends_with("<<EOF>>"), "tail dropped");
        assert!(
            !content_fact.contains("..."),
            "fact still truncated: {content_fact}"
        );
        assert_eq!(content_fact.len(), "content: ".len() + body.len());
    }
}
