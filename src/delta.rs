// SPDX-License-Identifier: Apache-2.0
//! Bridge: hifz mutation results → `memdiff::Change` → rendered diff.
//!
//! Two responsibilities, one place so REST, MCP and the web UI agree:
//! 1. `attach`/`attach_delta` — put the rendered `delta` on an API response.
//! 2. `record_observation` — for a *session-scoped* save, persist the delta
//!    as a `memory_delta` observation so it joins the existing session
//!    timeline and is replayable. No new table, no env vars.

use memdiff::{Change, MemoryDelta};
use serde_json::Value;

/// Render `changes` and attach as `delta` on a JSON object response.
pub fn attach(value: Value, changes: &[Change]) -> Value {
    attach_delta(value, memdiff::delta_from_changes(changes))
}

/// Attach an already-computed `delta` (avoids re-rendering when the caller
/// also recorded it). No-op if `value` is not a JSON object.
pub fn attach_delta(mut value: Value, delta: MemoryDelta) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("delta".into(), memdiff::sink_json::to_value(&delta));
    }
    value
}

/// Build the `Change` list for a save: always `Created`, plus `Superseded`
/// / `Linked(closes)` when the request asked for them.
pub fn save_changes(
    id: &str,
    title: &str,
    category: &str,
    supersedes: Option<&str>,
    closes: Option<&str>,
) -> Vec<Change> {
    let mut changes = vec![Change::Created {
        id: id.to_string(),
        title: title.to_string(),
        category: category.to_string(),
    }];
    if let Some(old) = supersedes {
        changes.push(Change::Superseded {
            old_id: old.to_string(),
            new_id: id.to_string(),
        });
    }
    if let Some(old) = closes {
        changes.push(Change::Linked {
            from: id.to_string(),
            to: old.to_string(),
            relation: "closes".into(),
            score: 1.0,
            reason: Some("explicit close".into()),
            via: "system".into(),
        });
    }
    changes
}

/// Build the `Change` list describing a memory's lineage for the inspect
/// view: the rows it superseded, its outgoing links, and its evolution
/// history. `links_json` is the `{"links":[...]}` shape from
/// [`crate::link::list_for`].
pub fn view_changes(
    memory_id: &str,
    superseded: &[String],
    links_json: &Value,
    history: &[crate::models::EvolutionEntry],
) -> Vec<Change> {
    let mut changes = Vec::new();
    for old in superseded {
        changes.push(Change::Superseded {
            old_id: old.clone(),
            new_id: memory_id.to_string(),
        });
    }
    if let Some(links) = links_json.get("links").and_then(|l| l.as_array()) {
        for l in links {
            changes.push(Change::Linked {
                from: memory_id.to_string(),
                to: l
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                relation: l
                    .get("relation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("related")
                    .to_string(),
                score: l.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
                reason: l
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                via: l
                    .get("via")
                    .and_then(|v| v.as_str())
                    .unwrap_or("system")
                    .to_string(),
            });
        }
    }
    for e in history {
        changes.push(Change::SelfRevised {
            id: memory_id.to_string(),
            field: e.field.clone(),
            previous: e.previous.clone(),
        });
    }
    changes
}

/// One-line human summary of a delta, for the observation `title`.
fn summarize(delta: &MemoryDelta) -> String {
    use memdiff::ChangeOp::*;
    let mut parts: Vec<String> = Vec::new();
    for line in &delta.lines {
        let word = match line.op {
            Created => "created",
            Revised => "revised",
            Superseded => "superseded",
            Linked => "linked",
            NeighbourRevised => "neighbour-revised",
            Forgotten => "forgotten",
            Conflict => "conflict",
        };
        let tag = format!("{} {word}", line.glyph.unicode());
        if !parts.contains(&tag) {
            parts.push(tag);
        }
    }
    let s = parts.join(" · ");
    if s.len() > 120 {
        format!("{}…", &s[..119])
    } else {
        s
    }
}

/// Record a session-scoped save's delta as a `memory_delta` observation so
/// it joins the existing session timeline / replay. Best-effort: never
/// fails the request. No-op without a session or for an empty delta.
pub async fn record_observation(
    db: &surrealdb::Surreal<crate::db::Db>,
    session_id: Option<&str>,
    delta: &MemoryDelta,
) {
    let Some(sid) = session_id.filter(|s| !s.is_empty()) else {
        return;
    };
    if delta.lines.is_empty() {
        return;
    }
    let session_rid = format!("session:{}", sid.strip_prefix("session:").unwrap_or(sid));
    let title = summarize(delta);
    let narrative =
        memdiff::sink_text::render(delta, &memdiff::sink_text::TextOpts { colour: false });
    let metadata = serde_json::json!({ "delta": memdiff::sink_json::to_value(delta) });
    let ts = chrono::Utc::now().to_rfc3339();

    // `ord` allocated inline; `obs_session_ord` UNIQUE is the race guard —
    // retry recomputes `count()` (mirrors `observe.rs`).
    let sql = "CREATE observation SET
            session_id = type::record($sid),
            ord        = count(SELECT id FROM observation WHERE session_id = type::record($sid)),
            source     = 'hifz',
            timestamp  = $ts,
            obs_type   = 'memory_delta',
            title      = $title,
            facts      = [],
            narrative  = $narrative,
            keywords   = [],
            files      = [],
            importance = 1,
            metadata   = $metadata
          RETURN id";

    for attempt in 0..4 {
        let res = db
            .query(sql)
            .bind(("sid", session_rid.clone()))
            .bind(("ts", ts.clone()))
            .bind(("title", title.clone()))
            .bind(("narrative", narrative.clone()))
            .bind(("metadata", metadata.clone()))
            .await
            .and_then(|r| r.check());
        match res {
            Ok(_) => return,
            Err(e) => {
                let m = e.to_string().to_lowercase();
                let conflict = m.contains("unique")
                    || m.contains("already contains")
                    || m.contains("conflict")
                    || m.contains("already exists");
                if conflict && attempt < 3 {
                    continue;
                }
                tracing::warn!("memory_delta observation record failed: {e}");
                return;
            }
        }
    }
}
