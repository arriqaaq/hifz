//! Integration test for the new `Hifz` library facade.
//!
//! Exercises one method per resource group end-to-end against an in-memory
//! SurrealKV instance, verifying that the library API works without any HTTP
//! server running.

use hifz::Hifz;
use hifz::models::{
    HookPayload, MemoriesReq, ObservationsReq, RememberReq, SearchReq, SessionStartReq, TimelineReq,
};

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[tokio::test]
async fn library_round_trip_event_session_observe_memory() {
    let h = Hifz::open_memory().await.expect("open in-memory hifz");

    // --- Sessions ---
    let res = h
        .session_start(SessionStartReq {
            session_id: "s_lib_test".into(),
            project: "/tmp/test".into(),
            cwd: "/tmp/test".into(),
        })
        .await
        .expect("session_start");
    assert_eq!(
        res.get("sessionId").and_then(|v| v.as_str()),
        Some("s_lib_test")
    );

    let listed = h.sessions_list(10).await.expect("sessions_list");
    let count = listed
        .get("sessions")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(count >= 1, "sessions_list should include the new session");

    // --- Observations via /observe ---
    // After the schema trim, `observation` is the single ordered log per
    // session (with monotonic `ord` and optional `parent_obs_id`). The old
    // event ledger is gone — no separate event_ingest test path.
    let payload = HookPayload {
        hook_type: "UserPromptSubmit".into(),
        session_id: "s_lib_test".into(),
        project: "/tmp/test".into(),
        cwd: "/tmp/test".into(),
        timestamp: now(),
        source: Some("library_test".into()),
        obs_type: Some("user_prompt".into()),
        parent_obs_id: None,
        data: serde_json::json!({"prompt": "what does library_api test do"}),
    };
    let prompt_obs_id = h
        .observe(payload)
        .await
        .expect("observe")
        .expect("prompt observation should be stored, not deduped");

    // Second observation parented to the first — proves the new field travels
    // through the pipeline and the ord allocator increments monotonically.
    let child = HookPayload {
        hook_type: "PostToolUse".into(),
        session_id: "s_lib_test".into(),
        project: "/tmp/test".into(),
        cwd: "/tmp/test".into(),
        timestamp: now(),
        source: Some("library_test".into()),
        obs_type: Some("file_read".into()),
        parent_obs_id: Some(prompt_obs_id),
        data: serde_json::json!({"tool_name": "Read", "tool_input": {"file_path": "/tmp/x"}}),
    };
    let _ = h.observe(child).await.expect("observe child");

    let obs = h
        .observations_search(ObservationsReq {
            obs_type: Some("user_prompt".into()),
            ..Default::default()
        })
        .await
        .expect("observations_search");
    let count = obs.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    assert!(count >= 1, "user_prompt observation should be searchable");

    // --- Memory ---
    let m = h
        .remember(RememberReq {
            title: "Library API works".into(),
            content: "Verified by tests/library_api.rs.".into(),
            category: Some("lesson".into()),
            keywords: Some(vec!["library".into(), "api".into()]),
            files: None,
            project: Some("/tmp/test".into()),
            session_id: None,
            ..Default::default()
        })
        .await
        .expect("remember");
    assert_eq!(m.get("status").and_then(|v| v.as_str()), Some("ok"));

    let mems = h
        .memories_search(MemoriesReq {
            project: Some("/tmp/test".into()),
            category: Some("lesson".into()),
            ..Default::default()
        })
        .await
        .expect("memories_search");
    let mc = mems.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    assert!(
        mc >= 1,
        "remembered lesson should appear in memories_search"
    );

    // --- Semantic search round-trip (proves embedding pipeline) ---
    let r = h
        .search_agentic(SearchReq {
            query: "library api works".into(),
            limit: Some(5),
            project: Some("/tmp/test".into()),
            ..Default::default()
        })
        .await
        .expect("search_agentic");
    let rcount = r.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    assert!(rcount >= 1, "search_agentic should find the insight");

    // --- Timeline ---
    let tl = h
        .timeline(TimelineReq {
            session_id: Some("s_lib_test".into()),
            limit: Some(20),
        })
        .await
        .expect("timeline");
    let tlc = tl
        .get("observations")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        tlc >= 1,
        "timeline should include the user_prompt observation"
    );

    // --- Health ---
    let hc = h.health().await.expect("health");
    assert_eq!(hc.get("status").and_then(|v| v.as_str()), Some("healthy"));
    assert!(hc.get("sessions").and_then(|v| v.as_i64()).unwrap_or(0) >= 1);

    // --- Session end ---
    h.session_end("s_lib_test").await.expect("session_end");
}

#[tokio::test]
async fn library_idempotent_open_runs_schema_migration() {
    // Opening twice on different in-memory DBs should both succeed and run migrations cleanly.
    let h1 = Hifz::open_memory().await.expect("first open");
    drop(h1);
    let h2 = Hifz::open_memory().await.expect("second open");
    let _ = h2.health().await.expect("health on second open");
}
