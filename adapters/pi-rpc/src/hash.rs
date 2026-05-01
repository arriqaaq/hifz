use sha2::{Digest, Sha256};

/// Deterministic event hash matching the TypeScript extension's algorithm.
pub fn hash_event(session_id: &str, event_type: &str, sequence: i64, payload: &serde_json::Value) -> String {
    let mut h = Sha256::new();
    h.update(session_id.as_bytes());
    h.update(b"\0");
    h.update(event_type.as_bytes());
    h.update(b"\0");
    if let Some(tcid) = payload.get("toolCallId").and_then(|v| v.as_str()) {
        h.update(tcid.as_bytes());
    } else {
        h.update(sequence.to_string().as_bytes());
    }
    format!("{:x}", h.finalize())
}
