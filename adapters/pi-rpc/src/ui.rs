use serde_json::json;

/// Build the response Pi expects for a given `extension_ui_request`.
///
/// Mirrors Pi's `noOpUIContext` semantics
/// (pi-mono/packages/coding-agent/src/core/extensions/runner.ts:188-219):
/// - interactive: select / confirm / input / editor → cancelled / false
/// - passive: notify / setStatus / setWidget / setTitle / set_editor_text → empty ack
pub fn auto_reply(req: &serde_json::Value, mode: &str) -> serde_json::Value {
    let id = req.get("id").cloned().unwrap_or(json!(""));
    let kind = req
        .get("request")
        .and_then(|r| r.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    match (mode, kind) {
        ("allow", "select") => {
            let first = req
                .get("request")
                .and_then(|r| r.get("options"))
                .and_then(|o| o.as_array())
                .and_then(|a| a.first())
                .cloned()
                .unwrap_or(json!(""));
            json!({"type": "extension_ui_response", "id": id, "value": first})
        }
        ("allow", "confirm") => {
            json!({"type": "extension_ui_response", "id": id, "confirmed": true})
        }
        ("allow", "input") | ("allow", "editor") => {
            json!({"type": "extension_ui_response", "id": id, "value": ""})
        }
        // Default: deny / cancel for interactive types.
        (_, "select") | (_, "input") | (_, "editor") => {
            json!({"type": "extension_ui_response", "id": id, "cancelled": true})
        }
        (_, "confirm") => {
            json!({"type": "extension_ui_response", "id": id, "confirmed": false})
        }
        // Passive: just ack.
        _ => json!({"type": "extension_ui_response", "id": id}),
    }
}
