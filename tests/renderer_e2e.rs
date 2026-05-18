//! End-to-end: the memdiff renderer wired through the `Hifz` facade.
//!
//! Verifies save → structured `delta`, session-scoped recording into the
//! `observation` timeline, replay list/get round-trip (recorded == live),
//! forget delta, and the inspect view — all against in-memory SurrealKV,
//! no HTTP server.

use hifz::Hifz;
use hifz::models::{RememberReq, SessionStartReq};

fn ops(v: &serde_json::Value) -> Vec<String> {
    v.get("delta")
        .and_then(|d| d.get("lines"))
        .and_then(|l| l.as_array())
        .map(|lines| {
            lines
                .iter()
                .filter_map(|ln| ln.get("op").and_then(|o| o.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn remember_req(title: &str, supersedes: Option<&str>) -> RememberReq {
    RememberReq {
        title: Some(title.into()),
        content: "body".into(),
        category: Some("decision".into()),
        keywords: None,
        files: None,
        tags: None,
        content_long: None,
        closes_memory_id: None,
        supersedes_memory_id: supersedes.map(String::from),
        project: Some("/tmp/rndr".into()),
        session_id: Some("s_rndr".into()),
    }
}

#[tokio::test]
async fn renderer_delta_record_replay_roundtrip() {
    let h = Hifz::open_memory().await.expect("open in-memory hifz");
    h.session_start(SessionStartReq {
        session_id: "s_rndr".into(),
        project: "/tmp/rndr".into(),
        cwd: "/tmp/rndr".into(),
    })
    .await
    .expect("session_start");

    // --- save A: a single `created` line ---
    let a = h
        .remember(remember_req("Auth uses JWT", None))
        .await
        .expect("remember A");
    assert_eq!(ops(&a), vec!["created"], "save A delta");
    let id_a = a
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id A")
        .to_string();

    // --- save B supersedes A: `created` + `superseded` ---
    let b = h
        .remember(remember_req("Auth uses JWT v2", Some(&id_a)))
        .await
        .expect("remember B");
    let b_ops = ops(&b);
    assert!(b_ops.contains(&"created".to_string()), "B has created");
    assert!(
        b_ops.contains(&"superseded".to_string()),
        "B has superseded"
    );
    let id_b = b
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id B")
        .to_string();

    // --- replay list shows the session with 2 recorded deltas ---
    let list = h.replays_list().await.expect("replays_list");
    let replays = list
        .get("replays")
        .and_then(|r| r.as_array())
        .expect("replays[]");
    let row = replays
        .iter()
        .find(|r| r.get("session_id").and_then(|v| v.as_str()) == Some("s_rndr"))
        .expect("session s_rndr listed");
    assert_eq!(row.get("count").and_then(|v| v.as_i64()), Some(2));

    // --- replay get: ordered events, recorded == live (determinism) ---
    let detail = h.replay_get("s_rndr").await.expect("replay_get");
    let events = detail
        .get("events")
        .and_then(|e| e.as_array())
        .expect("events[]");
    assert_eq!(events.len(), 2, "two recorded delta events");
    for ev in events {
        assert_eq!(ev.get("kind").and_then(|v| v.as_str()), Some("delta"));
    }
    // Event order is `ord` (save order): [0]=A, [1]=B. The recorded delta
    // must equal the live response delta byte-for-byte.
    assert_eq!(
        &events[0]["delta"],
        a.get("delta").unwrap(),
        "recorded A == live A"
    );
    assert_eq!(
        &events[1]["delta"],
        b.get("delta").unwrap(),
        "recorded B == live B"
    );

    // --- inspect view is coherent ---
    let view = h.memory_view(&id_b).await.expect("memory_view");
    assert!(
        view.get("header")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty())
    );
    assert!(
        view.get("rows").and_then(|v| v.as_array()).is_some(),
        "rows present"
    );

    // --- forget B: a `forgotten` line ---
    let f = h.forget(&id_b).await.expect("forget B");
    assert_eq!(ops(&f), vec!["forgotten"], "forget delta");
    assert_eq!(f.get("status").and_then(|v| v.as_str()), Some("ok"));
}
