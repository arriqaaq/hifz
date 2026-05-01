//! Event ledger — lossless raw event capture, no embedding.
//!
//! Producers (Pi extension, RPC adapter, future hooks) write every event here
//! for replay and audit. The dual-write companion is `/observe`, which embeds.
//!
//! All functions return `serde_json::Value` so the wire shape is bit-for-bit
//! identical to what the REST handlers used to produce inline.

use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::models::{EventRequest, EventsListReq};

/// Qualify a bare record id with its table prefix, leaving already-qualified
/// ids untouched. `qualify("session", "abc")` -> `"session:abc"`.
pub(crate) fn qualify_record(table: &str, raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("{table}:{raw}")
    }
}

/// Insert one event. Idempotent on `payload_hash` via the UNIQUE index —
/// duplicates return `{"status": "duplicate", "id": ...}`.
pub async fn ingest(db: &Surreal<Db>, ev: EventRequest) -> Result<serde_json::Value> {
    // Idempotency: if a row with this payload_hash already exists, return it.
    let mut existing = db
        .query("SELECT id FROM event WHERE payload_hash = $h LIMIT 1")
        .bind(("h", ev.payload_hash.clone()))
        .await?;
    let prior: Vec<serde_json::Value> = existing.take(0).unwrap_or_default();
    if let Some(row) = prior.into_iter().next() {
        return Ok(serde_json::json!({"status": "duplicate", "id": row.get("id").cloned()}));
    }

    // Build SET clause dynamically so optional record links are simply omitted
    // when None (rather than relying on inline IF/THEN/ELSE which doesn't
    // compose cleanly in SurrealQL SET).
    let mut sets: Vec<&'static str> = vec![
        "source = $source",
        "event_type = $event_type",
        "timestamp = $timestamp",
        "payload_hash = $payload_hash",
        "payload = $payload",
        "metadata = $metadata",
        "sequence = $sequence",
    ];
    if ev.session_id.is_some() {
        sets.push("session_id = type::record($session_id)");
    }
    if ev.run_id.is_some() {
        sets.push("run_id = type::record($run_id)");
    }
    if ev.parent_event_id.is_some() {
        sets.push("parent_event_id = type::record($parent_event_id)");
    }
    let sql = format!("CREATE event SET {}", sets.join(", "));

    let mut q = db
        .query(&sql)
        .bind(("source", ev.source))
        .bind(("event_type", ev.event_type))
        .bind(("sequence", ev.sequence))
        .bind(("timestamp", ev.timestamp))
        .bind(("payload_hash", ev.payload_hash))
        .bind(("payload", ev.payload))
        .bind(("metadata", ev.metadata));
    if let Some(s) = ev.session_id {
        q = q.bind(("session_id", qualify_record("session", &s)));
    }
    if let Some(r) = ev.run_id {
        q = q.bind(("run_id", qualify_record("run", &r)));
    }
    if let Some(p) = ev.parent_event_id {
        q = q.bind(("parent_event_id", qualify_record("event", &p)));
    }

    let mut resp = q.await?;
    let rows: Vec<serde_json::Value> = match resp.take(0) {
        Ok(v) => v,
        Err(e) => return Err(anyhow::anyhow!("event create returned error: {e}")),
    };
    match rows.into_iter().next() {
        Some(row) => Ok(serde_json::json!({"status": "ok", "id": row.get("id").cloned()})),
        None => Err(anyhow::anyhow!("event insert returned no row")),
    }
}

/// Insert a batch. Per-row failures don't fail the batch — each row's outcome
/// appears in the returned array.
pub async fn ingest_batch(
    db: &Surreal<Db>,
    events: Vec<EventRequest>,
) -> Result<serde_json::Value> {
    let mut results = Vec::with_capacity(events.len());
    for ev in events {
        let r = match ingest(db, ev).await {
            Ok(v) => v,
            Err(e) => serde_json::json!({"status": "error", "error": e.to_string()}),
        };
        results.push(r);
    }
    Ok(serde_json::json!({"results": results}))
}

/// List events with optional filters. Capped at 1000.
pub async fn list(db: &Surreal<Db>, params: EventsListReq) -> Result<serde_json::Value> {
    let limit = params.limit.unwrap_or(200).min(1000);

    let mut sql = String::from("SELECT * FROM event");
    let mut conds = Vec::<String>::new();

    if params.source.is_some() {
        conds.push("source = $source".into());
    }
    if params.event_type.is_some() {
        conds.push("event_type = $event_type".into());
    }
    if params.session_id.is_some() {
        conds.push("session_id = type::record($sid)".into());
    }
    if params.run_id.is_some() {
        conds.push("run_id = type::record($rid)".into());
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    sql.push_str(&format!(
        " ORDER BY sequence ASC, timestamp ASC LIMIT {limit}"
    ));

    let mut q = db.query(sql);
    if let Some(s) = params.source {
        q = q.bind(("source", s));
    }
    if let Some(t) = params.event_type {
        q = q.bind(("event_type", t));
    }
    if let Some(s) = params.session_id {
        q = q.bind(("sid", qualify_record("session", &s)));
    }
    if let Some(r) = params.run_id {
        q = q.bind(("rid", qualify_record("run", &r)));
    }

    let mut resp = q.await?;
    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let count = rows.len();
    Ok(serde_json::json!({"events": rows, "count": count}))
}

/// Fetch one event by id. Returns `{"error": "..."}` when not found, matching
/// the legacy handler shape.
pub async fn get(db: &Surreal<Db>, id: &str) -> Result<serde_json::Value> {
    let rid = qualify_record("event", id);
    let mut resp = db
        .query("SELECT * FROM type::record($rid)")
        .bind(("rid", rid))
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    match rows.into_iter().next() {
        Some(row) => Ok(row),
        None => Ok(serde_json::json!({"error": "event not found"})),
    }
}
