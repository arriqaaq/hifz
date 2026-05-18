// SPDX-License-Identifier: Apache-2.0
//! Replay wire contract. A recorded session is a list of [`SessionEvent`]s;
//! each embeds the *already-structured* `MemoryDelta` / `MemoryView`, so
//! replay is just re-rendering recorded events through the same sinks /
//! components as live — byte-identical, no recompute.
//!
//! Persistence is hifz's existing `observation` timeline (one
//! `obs_type="memory_delta"` row per session-scoped save, the delta in
//! `metadata.delta`); this crate only owns the serialization shape.
//! Timestamps are caller-supplied strings (the recording side owns the
//! clock — this crate stays dependency-free).

use serde::{Deserialize, Serialize};

use crate::model::{MemoryDelta, MemoryView};

/// One recorded event. The replay API returns `Delta` events built from
/// `memory_delta` observations; `Prompt`/`Note`/`Error`/`View` are for
/// richer recordings and the player's transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    Prompt { t: String, text: String },
    Delta { t: String, delta: MemoryDelta },
    View { t: String, view: MemoryView },
    Note { t: String, text: String },
    Error { t: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::delta_from_changes;
    use crate::model::Change;
    use crate::sink_text::{TextOpts, render};

    #[test]
    fn delta_event_round_trips_and_re_renders_identically() {
        let delta = delta_from_changes(&[
            Change::Created {
                id: "memory:a".into(),
                title: "Auth uses JWT".into(),
                category: "decision".into(),
            },
            Change::Superseded {
                old_id: "memory:a".into(),
                new_id: "memory:b".into(),
            },
        ]);
        let ev = SessionEvent::Delta {
            t: "2026-05-18T14:30:00Z".into(),
            delta: delta.clone(),
        };

        // The wire form a replay API row carries, round-tripped.
        let json = serde_json::to_string(&ev).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ev);

        if let SessionEvent::Delta {
            delta: recorded, ..
        } = back
        {
            let opts = TextOpts { colour: false };
            assert_eq!(
                render(&recorded, &opts),
                render(&delta, &opts),
                "replay render must equal live render"
            );
        } else {
            panic!("expected Delta");
        }
    }
}
