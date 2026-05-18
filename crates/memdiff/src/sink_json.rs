// SPDX-License-Identifier: Apache-2.0
//! JSON sink. The `MemoryDelta` *is* the schema — this is a thin, stable
//! serialization the REST/MCP layers embed and the web UI deserializes.
//!
//! Schema (stable): `{ "lines": [ { "op": <op>, "glyph": <glyph>,
//! "spans": [ { "text": str, "style": { "tone": <tone>, "bold"?, "dim"?,
//! "strike"? }, "cite"?: { "kind": "memory|edge|run", … } } ] } ] }`.

use crate::model::{MemoryDelta, MemoryView};

/// `MemoryDelta` → `serde_json::Value` (never panics; empty delta on error).
pub fn to_value(delta: &MemoryDelta) -> serde_json::Value {
    serde_json::to_value(delta).unwrap_or_else(|_| serde_json::json!({ "lines": [] }))
}

/// `MemoryView` → `serde_json::Value`.
pub fn view_to_value(view: &MemoryView) -> serde_json::Value {
    serde_json::to_value(view).unwrap_or_else(|_| serde_json::json!({ "header": [], "rows": [] }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::delta_from_changes;
    use crate::model::Change;

    #[test]
    fn round_trips_through_json() {
        let d = delta_from_changes(&[Change::Forgotten {
            id: "memory:z".into(),
        }]);
        let v = to_value(&d);
        let back: MemoryDelta = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(d, back);
        // op tag is the stable wire contract the UI keys off.
        assert_eq!(v["lines"][0]["op"], "forgotten");
    }
}
