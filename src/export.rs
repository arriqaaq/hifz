//! Export — bundle every relevant table into one JSON for backup or migration.

use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::models::ExportReq;

/// Build the export bundle. Filters apply to the observation slice; sessions,
/// memories etc. are filtered only by project (when supplied).
pub async fn run(db: &Surreal<Db>, params: ExportReq) -> Result<serde_json::Value> {
    // Observations — full filter set.
    let mut obs_conditions: Vec<String> = Vec::new();
    if let Some(ref sid) = params.session_id {
        let sid_clean = sid
            .strip_prefix("session:")
            .unwrap_or(sid)
            .replace('\'', "");
        obs_conditions.push(format!(
            "session_id = type::record('session:{}')",
            sid_clean
        ));
    }
    if params.project.is_some() {
        obs_conditions.push("project = $project".to_string());
    }
    if let Some(ref types) = params.obs_type {
        let parts: Vec<String> = types
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| format!("'{}'", s.replace('\'', "")))
            .collect();
        if !parts.is_empty() {
            obs_conditions.push(format!("obs_type IN [{}]", parts.join(", ")));
        }
    }
    if params.since.is_some() {
        obs_conditions.push("timestamp >= $since".to_string());
    }
    if params.until.is_some() {
        obs_conditions.push("timestamp <= $until".to_string());
    }
    if let Some(min_imp) = params.min_importance {
        obs_conditions.push(format!("importance >= {}", min_imp));
    }
    let obs_where = if obs_conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", obs_conditions.join(" AND "))
    };
    let obs_sql = format!("SELECT * FROM observation{}", obs_where);

    let mut obs_q = db.query(&obs_sql);
    if let Some(ref project) = params.project {
        obs_q = obs_q.bind(("project", project.clone()));
    }
    if let Some(ref since) = params.since {
        obs_q = obs_q.bind(("since", since.clone()));
    }
    if let Some(ref until) = params.until {
        obs_q = obs_q.bind(("until", until.clone()));
    }
    let observations: Vec<serde_json::Value> = obs_q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    // Sessions — project filter only.
    let sessions_sql = if params.project.is_some() {
        "SELECT * FROM session WHERE project = $project"
    } else {
        "SELECT * FROM session"
    };
    let mut s_q = db.query(sessions_sql);
    if let Some(ref project) = params.project {
        s_q = s_q.bind(("project", project.clone()));
    }
    let sessions: Vec<serde_json::Value> = s_q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    // Memories — project filter with global fallback.
    let memories_sql = if params.project.is_some() {
        "SELECT * FROM memory WHERE is_latest = true AND (project = $project OR project = 'global')"
    } else {
        "SELECT * FROM memory WHERE is_latest = true"
    };
    let mut m_q = db.query(memories_sql);
    if let Some(ref project) = params.project {
        m_q = m_q.bind(("project", project.clone()));
    }
    let memories: Vec<serde_json::Value> = m_q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    // Consolidation outputs live in `memory` with category='semantic_fact' or
    // 'procedure'; surface them in their own export buckets for tooling that
    // expects the old shape.
    let semantic: Vec<serde_json::Value> = db
        .query("SELECT * FROM memory WHERE category = 'semantic_fact'")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let procedural: Vec<serde_json::Value> = db
        .query("SELECT * FROM memory WHERE category = 'procedure'")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let runs: Vec<serde_json::Value> = db
        .query("SELECT * FROM run ORDER BY started_at DESC")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let commits: Vec<serde_json::Value> = db
        .query("SELECT * FROM observation WHERE obs_type = 'commit_made' ORDER BY timestamp DESC")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    // Edge table — used to derive `DISTILLED_FROM` between memories.
    let edge_rows: Vec<serde_json::Value> = db
        .query("SELECT in, out, relation FROM edge")
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();

    let nodes = build_nodes(&sessions, &runs, &observations, &memories);
    let edges = build_edges(&runs, &observations, &edge_rows);

    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "sessions": sessions,
        "observations": observations,
        "memories": memories,
        "semantic_memories": semantic,
        "procedural_memories": procedural,
        "runs": runs,
        "commits": commits,
        "nodes": nodes,
        "edges": edges,
    }))
}

// ---------------------------------------------------------------------------
// Graph projection — typed nodes + relationship edges
// ---------------------------------------------------------------------------

/// Convert a SurrealDB record-id JSON value to its canonical `"table:key"`
/// string. Handles every shape the SDK has emitted across versions.
fn record_str(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    let obj = v.as_object()?;
    let tb = obj.get("tb").and_then(|t| t.as_str());

    // Pull the inner key out of either `id` (newer SDK) or `key` (legacy).
    let key_val = obj.get("id").or_else(|| obj.get("key"))?;
    let key = match key_val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(inner) => {
            // {String: "..."} or {Number: 42}
            if let Some(s) = inner.get("String").and_then(|s| s.as_str()) {
                s.to_string()
            } else if let Some(n) = inner.get("Number").and_then(|n| n.as_i64()) {
                n.to_string()
            } else {
                return None;
            }
        }
        _ => return None,
    };
    match tb {
        Some(tb) => Some(format!("{tb}:{key}")),
        None => Some(key),
    }
}

fn label_from(row: &serde_json::Value, fields: &[&str]) -> String {
    for field in fields {
        if let Some(s) = row.get(*field).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn build_nodes(
    sessions: &[serde_json::Value],
    runs: &[serde_json::Value],
    observations: &[serde_json::Value],
    memories: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();

    for s in sessions {
        if let Some(id) = s.get("id").and_then(record_str) {
            let label = label_from(s, &["name", "project"]);
            out.push(serde_json::json!({"id": id, "kind": "session", "label": label}));
        }
    }
    for r in runs {
        if let Some(id) = r.get("id").and_then(record_str) {
            let label = label_from(r, &["prompt"]);
            out.push(serde_json::json!({"id": id, "kind": "run", "label": label}));
        }
    }
    for o in observations {
        // Skip noise that the graph view has always filtered out.
        let title = label_from(o, &["title"]);
        let obs_type = o.get("obs_type").and_then(|v| v.as_str()).unwrap_or("");
        if title == "unknown call" || obs_type == "conversation" {
            continue;
        }
        if let Some(id) = o.get("id").and_then(record_str) {
            let kind = if obs_type == "commit_made" {
                "commit"
            } else {
                "observation"
            };
            out.push(serde_json::json!({"id": id, "kind": kind, "label": title}));
        }
    }
    for m in memories {
        if let Some(id) = m.get("id").and_then(record_str) {
            let label = label_from(m, &["title"]);
            out.push(serde_json::json!({"id": id, "kind": "memory", "label": label}));
        }
    }
    out
}

fn build_edges(
    runs: &[serde_json::Value],
    observations: &[serde_json::Value],
    edge_rows: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();

    // IN_SESSION : run -> session
    // IN_RUN     : observation -> run
    // RECALLS    : run -> memory
    for r in runs {
        let run_id = match r.get("id").and_then(record_str) {
            Some(s) => s,
            None => continue,
        };
        if let Some(sid) = r.get("session_id").and_then(record_str) {
            out.push(serde_json::json!({
                "source": run_id, "target": sid, "rel": "IN_SESSION",
            }));
        }
        if let Some(arr) = r.get("observation_ids").and_then(|v| v.as_array()) {
            for o in arr {
                if let Some(oid) = record_str(o) {
                    out.push(serde_json::json!({
                        "source": oid, "target": run_id.clone(), "rel": "IN_RUN",
                    }));
                }
            }
        }
        if let Some(arr) = r.get("recalled_ids").and_then(|v| v.as_array()) {
            for m in arr {
                if let Some(mid) = record_str(m) {
                    out.push(serde_json::json!({
                        "source": run_id.clone(), "target": mid, "rel": "RECALLS",
                    }));
                }
            }
        }
    }

    // PRODUCED_BY : commit observation -> run within same session,
    // closest run by ended_at within ±5 min.
    let window_ms: i64 = 5 * 60 * 1000;
    let parse_ts = |s: &str| -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.timestamp_millis())
    };
    for o in observations {
        if o.get("obs_type").and_then(|v| v.as_str()) != Some("commit_made") {
            continue;
        }
        let cid = match o.get("id").and_then(record_str) {
            Some(s) => s,
            None => continue,
        };
        let csid = match o.get("session_id").and_then(record_str) {
            Some(s) => s,
            None => continue,
        };
        let cts = match o
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_ts)
        {
            Some(t) => t,
            None => continue,
        };

        let mut best: Option<(String, i64)> = None;
        for r in runs {
            if r.get("session_id").and_then(record_str).as_deref() != Some(&csid) {
                continue;
            }
            let rid = match r.get("id").and_then(record_str) {
                Some(s) => s,
                None => continue,
            };
            let rts = r
                .get("ended_at")
                .and_then(|v| v.as_str())
                .or_else(|| r.get("started_at").and_then(|v| v.as_str()))
                .and_then(parse_ts);
            if let Some(rts) = rts {
                let dist = (cts - rts).abs();
                if dist <= window_ms && best.as_ref().is_none_or(|(_, d)| dist < *d) {
                    best = Some((rid, dist));
                }
            }
        }
        if let Some((rid, _)) = best {
            out.push(serde_json::json!({
                "source": cid, "target": rid, "rel": "PRODUCED_BY",
            }));
        }
    }

    // DISTILLED_FROM : memory -> memory derived from `edge` table rows whose
    // `relation` is one of the provenance/co-occurrence kinds. Only edges
    // where both endpoints belong to the `memory` table are surfaced.
    for e in edge_rows {
        let rel = e.get("relation").and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(
            rel,
            "co_occurs_embedding"
                | "co_occurs_keywords"
                | "co_occurs_files"
                | "derived_from"
                | "generated_by"
                | "related"
                | "elaborates"
        ) {
            continue;
        }
        let src = match e.get("in").and_then(record_str) {
            Some(s) if s.starts_with("memory:") => s,
            _ => continue,
        };
        let dst = match e.get("out").and_then(record_str) {
            Some(s) if s.starts_with("memory:") => s,
            _ => continue,
        };
        out.push(serde_json::json!({
            "source": src, "target": dst, "rel": "DISTILLED_FROM",
        }));
    }

    // SHARES_FILE : observation pairs in the same session that share ≥1 file.
    // Capped at the top 200 strongest pairs per session to keep payload small.
    use std::collections::HashMap;
    let mut by_session: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
    for o in observations {
        if let Some(sid) = o.get("session_id").and_then(record_str) {
            by_session.entry(sid).or_default().push(o);
        }
    }
    for (_, group) in by_session {
        // ids + file sets
        let mut prepared: Vec<(String, std::collections::HashSet<String>)> = Vec::new();
        for o in &group {
            let oid = match o.get("id").and_then(record_str) {
                Some(s) => s,
                None => continue,
            };
            let files: std::collections::HashSet<String> = o
                .get("files")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|f| f.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if !files.is_empty() {
                prepared.push((oid, files));
            }
        }
        let mut pairs: Vec<(String, String, usize)> = Vec::new();
        for i in 0..prepared.len() {
            for j in (i + 1)..prepared.len() {
                let shared = prepared[i].1.intersection(&prepared[j].1).count();
                if shared > 0 {
                    pairs.push((prepared[i].0.clone(), prepared[j].0.clone(), shared));
                }
            }
        }
        pairs.sort_by(|a, b| b.2.cmp(&a.2));
        for (a, b, _) in pairs.into_iter().take(200) {
            out.push(serde_json::json!({
                "source": a, "target": b, "rel": "SHARES_FILE",
            }));
        }
    }

    out
}
