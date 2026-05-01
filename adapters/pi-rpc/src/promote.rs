use serde_json::{json, Value};

/// Returns Some(HookPayload-shaped JSON) for events that should also flow through
/// `/observe`, or None for events that stay in the lossless ledger only.
///
/// Mirrors `adapters/pi-extension/src/promote.ts` and uses the same Hifz hookType
/// vocabulary recognized by `src/models.rs From<&str> for HifzEvent`.
pub fn promote(event_type: &str, evt: &Value, project: &str, session_id: &str) -> Option<Value> {
    let base = json!({
        "sessionId": session_id,
        "project": project,
        "cwd": project,
        "timestamp": iso_now(),
        "source": "pi_rpc",
    });

    match event_type {
        "turn_start" => Some(merge(
            base,
            json!({
                "hookType": "UserPromptSubmit",
                "obs_type": "user_prompt",
                "data": { "turnIndex": evt.get("turnIndex"), "evt": evt },
            }),
        )),
        "tool_execution_end" => {
            let is_error = evt.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
            let tool_name = evt.get("toolName").and_then(|v| v.as_str()).unwrap_or("");
            Some(merge(
                base,
                json!({
                    "hookType": if is_error { "PostToolUseFailure" } else { "PostToolUse" },
                    "obs_type": if is_error { "error" } else { tool_to_obs_type(tool_name) },
                    "data": {
                        "tool_name": tool_name,
                        "tool_input": evt.get("args"),
                        "tool_output": truncate(evt.get("result").cloned().unwrap_or(Value::Null)),
                        "is_error": is_error,
                        "toolCallId": evt.get("toolCallId"),
                    },
                }),
            ))
        }
        "compaction_end" => {
            let result = evt.get("result");
            let aborted = evt.get("aborted").and_then(|v| v.as_bool()).unwrap_or(false);
            if result.is_none() || aborted {
                return None;
            }
            Some(merge(
                base,
                json!({
                    "hookType": "PostCompact",
                    "obs_type": "compaction_summary",
                    "data": { "result": result, "reason": evt.get("reason") },
                }),
            ))
        }
        "auto_retry_end" => {
            let success = evt.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
            if success {
                return None;
            }
            Some(merge(
                base,
                json!({
                    "hookType": "PostToolUseFailure",
                    "obs_type": "error",
                    "data": { "final_error": evt.get("finalError"), "attempt": evt.get("attempt") },
                }),
            ))
        }
        "turn_end" | "agent_end" => Some(merge(
            base,
            json!({
                "hookType": "Stop",
                "data": { "evt": evt },
            }),
        )),
        _ => None,
    }
}

fn tool_to_obs_type(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "read" => "file_read",
        "write" => "file_write",
        "edit" => "file_edit",
        "bash" => "command_run",
        "grep" | "find" | "ls" => "search",
        _ => "tool_use",
    }
}

const OBSERVE_MAX_BYTES: usize = 8 * 1024;

fn truncate(v: Value) -> Value {
    let s = match serde_json::to_string(&v) {
        Ok(s) => s,
        Err(_) => return v,
    };
    if s.len() <= OBSERVE_MAX_BYTES {
        v
    } else {
        json!({ "__truncated": true, "sample": &s[..OBSERVE_MAX_BYTES] })
    }
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Value::Object(ref mut am), Value::Object(bm)) = (&mut a, b) {
        for (k, v) in bm {
            am.insert(k, v);
        }
    }
    a
}

fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339()
}
