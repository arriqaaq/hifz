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

/// Bug 2 + 3a: every distinct prompt in a session must persist with a real
/// `observation:<id>` (not the title), and must be appended to the open run.
/// Pre-fix: the dedup hash collided on every prompt (only the first stored),
/// and the transactional id capture returned None (title as obs_id, run
/// append skipped).
#[tokio::test]
async fn bug2_3a_distinct_prompts_persist_with_real_ids_and_run_link() {
    let h = Hifz::open_memory().await.expect("open in-memory hifz");
    h.session_start(SessionStartReq {
        session_id: "s_bug2".into(),
        project: "/tmp/bug2".into(),
        cwd: "/tmp/bug2".into(),
    })
    .await
    .expect("session_start");

    let mk = |prompt: &str| HookPayload {
        hook_type: "UserPromptSubmit".into(),
        session_id: "s_bug2".into(),
        project: "/tmp/bug2".into(),
        cwd: "/tmp/bug2".into(),
        timestamp: now(),
        source: Some("library_test".into()),
        obs_type: Some("user_prompt".into()),
        parent_obs_id: None,
        data: serde_json::json!({ "prompt": prompt }),
    };

    let id_a = h
        .observe(mk("alpha: investigate the widget subsystem"))
        .await
        .expect("observe alpha")
        .expect("alpha prompt must be stored, not deduped");
    let id_b = h
        .observe(mk("beta: a completely different prompt about gadgets"))
        .await
        .expect("observe beta")
        .expect("beta prompt must be stored, not deduped (Bug 3a)");

    assert!(
        id_a.starts_with("observation:"),
        "obs_id must be a record id, got {id_a:?} (Bug 2)"
    );
    assert!(
        id_b.starts_with("observation:"),
        "obs_id must be a record id, got {id_b:?} (Bug 2)"
    );
    assert_ne!(id_a, id_b, "two distinct prompts must yield distinct ids");

    let obs = h
        .observations_search(ObservationsReq {
            session_id: Some("s_bug2".into()),
            ..Default::default()
        })
        .await
        .expect("observations_search");
    assert_eq!(
        obs.get("count").and_then(|v| v.as_u64()),
        Some(2),
        "both prompt observations must be searchable (Bug 3a)"
    );

    // Run linkage proves the `if let Some(obs_id)` block executed — i.e.
    // new_obs_id was actually captured (Bug 2).
    let tree = h.session_tree("s_bug2").await.expect("session_tree");
    let linked: usize = tree
        .get("runs")
        .and_then(|v| v.as_array())
        .map(|runs| {
            runs.iter()
                .filter_map(|r| r.get("observation_ids").and_then(|v| v.as_array()))
                .map(|a| a.len())
                .sum()
        })
        .unwrap_or(0);
    assert!(
        linked >= 2,
        "both observations must be appended to the open run (Bug 2), got {linked}"
    );
}

/// Bug 3b: a malformed `parent_obs_id` (e.g. a leaked title with colons)
/// must NOT fail the whole observation — it degrades to no parent. Pre-fix
/// it was fed to `type::record` and violated the schema, dropping every
/// PostToolUse observation.
#[tokio::test]
async fn bug3b_malformed_parent_does_not_drop_observation() {
    let h = Hifz::open_memory().await.expect("open in-memory hifz");
    h.session_start(SessionStartReq {
        session_id: "s_bug3b".into(),
        project: "/tmp/bug3b".into(),
        cwd: "/tmp/bug3b".into(),
    })
    .await
    .expect("session_start");

    let prompt_id = h
        .observe(HookPayload {
            hook_type: "UserPromptSubmit".into(),
            session_id: "s_bug3b".into(),
            project: "/tmp/bug3b".into(),
            cwd: "/tmp/bug3b".into(),
            timestamp: now(),
            source: Some("library_test".into()),
            obs_type: Some("user_prompt".into()),
            parent_obs_id: None,
            data: serde_json::json!({ "prompt": "kick off the session" }),
        })
        .await
        .expect("observe prompt")
        .expect("prompt stored");
    assert!(prompt_id.starts_with("observation:"));

    // Valid parent → links fine.
    h.observe(HookPayload {
        hook_type: "PostToolUse".into(),
        session_id: "s_bug3b".into(),
        project: "/tmp/bug3b".into(),
        cwd: "/tmp/bug3b".into(),
        timestamp: now(),
        source: Some("library_test".into()),
        obs_type: Some("file_read".into()),
        parent_obs_id: Some(prompt_id),
        data: serde_json::json!({ "tool_name": "Read", "tool_input": { "file_path": "/a" } }),
    })
    .await
    .expect("observe with valid parent")
    .expect("valid-parent tool obs stored");

    // Malformed parent (leaked title with colons) → must still store.
    let bogus = h
        .observe(HookPayload {
            hook_type: "PostToolUse".into(),
            session_id: "s_bug3b".into(),
            project: "/tmp/bug3b".into(),
            cwd: "/tmp/bug3b".into(),
            timestamp: now(),
            source: Some("library_test".into()),
            obs_type: Some("file_read".into()),
            parent_obs_id: Some("Prompt: <ide_opened_file> /tmp/x:y (junk):with:colons".into()),
            data: serde_json::json!({ "tool_name": "Read", "tool_input": { "file_path": "/b" } }),
        })
        .await
        .expect("observe with malformed parent must not error (Bug 3b)")
        .expect("malformed-parent tool obs must still be stored (Bug 3b)");
    assert!(
        bogus.starts_with("observation:"),
        "malformed parent must degrade to NONE, not drop the row, got {bogus:?}"
    );
}

/// Bug 1: the project filter on `observations_search` must scope by the
/// linked session's project (`session_id.project`), not a non-existent
/// `project` field on the observation row.
#[tokio::test]
async fn bug1_observations_project_filter_traverses_session() {
    let h = Hifz::open_memory().await.expect("open in-memory hifz");
    for (sid, proj) in [("s_pa", "/tmp/projA"), ("s_pb", "/tmp/projB")] {
        h.session_start(SessionStartReq {
            session_id: sid.into(),
            project: proj.into(),
            cwd: proj.into(),
        })
        .await
        .expect("session_start");
        h.observe(HookPayload {
            hook_type: "UserPromptSubmit".into(),
            session_id: sid.into(),
            project: proj.into(),
            cwd: proj.into(),
            timestamp: now(),
            source: Some("library_test".into()),
            obs_type: Some("user_prompt".into()),
            parent_obs_id: None,
            data: serde_json::json!({ "prompt": format!("prompt in {proj}") }),
        })
        .await
        .expect("observe")
        .expect("stored");
    }

    let only_a = h
        .observations_search(ObservationsReq {
            project: Some("/tmp/projA".into()),
            ..Default::default()
        })
        .await
        .expect("observations_search projA");
    assert_eq!(
        only_a.get("count").and_then(|v| v.as_u64()),
        Some(1),
        "project filter must return only projA's observation (Bug 1)"
    );

    let unfiltered = h
        .observations_search(ObservationsReq::default())
        .await
        .expect("observations_search all");
    assert!(
        unfiltered
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            >= 2,
        "unfiltered search must see both observations"
    );
}

#[tokio::test]
async fn library_idempotent_open_runs_schema_migration() {
    // Opening twice on different in-memory DBs should both succeed and run migrations cleanly.
    let h1 = Hifz::open_memory().await.expect("first open");
    drop(h1);
    let h2 = Hifz::open_memory().await.expect("second open");
    let _ = h2.health().await.expect("health on second open");
}
