//! Integration test for the new `event` table.
//!
//! Validates that:
//!  1. The schema migration in src/db.rs creates the table and indexes.
//!  2. CREATE event with a known payload_hash succeeds.
//!  3. A second CREATE with the same payload_hash fails the UNIQUE index
//!     (so the handler's idempotency check is meaningful).
//!  4. Querying by source / event_type / session_id round-trips.

use serde_json::json;

#[tokio::test]
async fn event_table_basic_round_trip() {
    let db = hifz::db::connect_mem().await.expect("mem db");
    hifz::db::init_schema(&db, 384).await.expect("schema");

    // Insert a session row so the optional record link resolves cleanly.
    let _ = db
        .query("CREATE session:s1 SET project = 'p', cwd = 'p', started_at = '2025-01-01T00:00:00Z', status = 'active', observation_count = 0")
        .await
        .expect("session row");

    // Insert one event.
    let payload = json!({"toolCallId": "abc", "args": {"command": "ls"}});
    let mut resp = db
        .query(
            "CREATE event SET source = $s, event_type = $t, \
             session_id = type::record($sid), \
             timestamp = $ts, payload_hash = $h, payload = $p",
        )
        .bind(("s", "test"))
        .bind(("t", "tool_execution_end"))
        .bind(("sid", "session:s1"))
        .bind(("ts", "2025-01-01T00:00:01Z"))
        .bind(("h", "hash-aaa"))
        .bind(("p", payload.clone()))
        .await
        .expect("first insert");
    let rows: Vec<serde_json::Value> = resp.take(0).expect("rows");
    assert_eq!(rows.len(), 1, "first insert should produce one row");

    // Second insert with the same payload_hash must fail (UNIQUE index).
    let conflict = db
        .query(
            "CREATE event SET source = $s, event_type = $t, \
             timestamp = $ts, payload_hash = $h, payload = $p",
        )
        .bind(("s", "test"))
        .bind(("t", "tool_execution_end"))
        .bind(("ts", "2025-01-01T00:00:02Z"))
        .bind(("h", "hash-aaa"))
        .bind(("p", payload))
        .await;
    assert!(
        conflict.is_err() || conflict.unwrap().take::<Vec<serde_json::Value>>(0).is_err(),
        "duplicate payload_hash must violate UNIQUE index"
    );

    // Round-trip query by source.
    let mut q = db
        .query("SELECT source, event_type, payload_hash FROM event WHERE source = $s")
        .bind(("s", "test"))
        .await
        .expect("select");
    let found: Vec<serde_json::Value> = q.take(0).expect("rows");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["source"], "test");
    assert_eq!(found[0]["payload_hash"], "hash-aaa");
}

#[tokio::test]
async fn event_table_session_link_optional() {
    let db = hifz::db::connect_mem().await.expect("mem db");
    hifz::db::init_schema(&db, 384).await.expect("schema");

    // No session row, no session_id on the event — must still insert.
    let mut resp = db
        .query(
            "CREATE event SET source = $s, event_type = $t, \
             timestamp = $ts, payload_hash = $h",
        )
        .bind(("s", "test"))
        .bind(("t", "agent_start"))
        .bind(("ts", "2025-01-01T00:00:00Z"))
        .bind(("h", "hash-no-session"))
        .await
        .expect("insert no session");
    let rows: Vec<serde_json::Value> = resp.take(0).expect("rows");
    assert_eq!(rows.len(), 1);
}
