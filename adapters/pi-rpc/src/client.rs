//! In-process Hifz client. Replaces the previous reqwest-based HTTP client —
//! every method now calls the Hifz library facade directly.
//!
//! The harness still passes data as `serde_json::Value`, so this layer
//! deserialises into the typed request structs from `hifz::models` before
//! invoking the methods. On failure we spool the raw value to disk for replay.

use std::sync::Arc;

use hifz::Hifz;
use hifz::models::{EventRequest, HookPayload, SessionStartReq};

use crate::spool::Spool;

#[derive(Clone)]
pub struct Client {
    hifz: Hifz,
    spool: Arc<Spool>,
}

impl Client {
    /// Open a persistent Hifz at `db_path`. Schema migration runs on first open.
    pub async fn new(db_path: &str, spool: Spool) -> anyhow::Result<Self> {
        let hifz = Hifz::open_persistent(db_path).await?;
        Ok(Self {
            hifz,
            spool: Arc::new(spool),
        })
    }

    pub async fn send_event(&self, body: &serde_json::Value) {
        match serde_json::from_value::<EventRequest>(body.clone()) {
            Ok(ev) => {
                if let Err(e) = self.hifz.event_ingest(ev).await {
                    tracing::warn!("event_ingest failed (spooling): {e}");
                    let _ = self.spool.append("event", body);
                }
            }
            Err(e) => {
                tracing::warn!("event payload deserialise failed (spooling): {e}");
                let _ = self.spool.append("event", body);
            }
        }
    }

    pub async fn send_observation(&self, body: &serde_json::Value) {
        match serde_json::from_value::<HookPayload>(body.clone()) {
            Ok(payload) => {
                if let Err(e) = self.hifz.observe(payload).await {
                    tracing::warn!("observe failed (spooling): {e}");
                    let _ = self.spool.append("observation", body);
                }
            }
            Err(e) => {
                tracing::warn!("observation payload deserialise failed (spooling): {e}");
                let _ = self.spool.append("observation", body);
            }
        }
    }

    pub async fn start_session(
        &self,
        body: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let req: SessionStartReq = serde_json::from_value(body.clone())?;
        self.hifz.session_start(req).await
    }

    pub async fn end_session(&self, body: &serde_json::Value) {
        let session_id = body
            .get("sessionId")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("session_id").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if session_id.is_empty() {
            tracing::warn!("end_session: no sessionId in body, skipping");
            return;
        }
        if let Err(e) = self.hifz.session_end(&session_id).await {
            tracing::warn!("session_end failed: {e}");
        }
    }

    /// Replay any spooled events. Each kind dispatches into the corresponding
    /// library method. On the first failure we stop and remain degraded; the
    /// remaining files persist for the next call.
    pub async fn drain_spool(&self) {
        let drained = match self.spool.drain() {
            Ok(d) => d,
            Err(_) => return,
        };
        let mut files_to_remove = std::collections::HashSet::new();
        for (file, line) in drained {
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(&line);
            let Ok(v) = parsed else { continue };
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let body = v.get("body").cloned().unwrap_or(serde_json::Value::Null);
            let ok = match kind {
                "event" => match serde_json::from_value::<EventRequest>(body.clone()) {
                    Ok(ev) => self.hifz.event_ingest(ev).await.is_ok(),
                    Err(_) => false,
                },
                "observation" => match serde_json::from_value::<HookPayload>(body.clone()) {
                    Ok(p) => self.hifz.observe(p).await.is_ok(),
                    Err(_) => false,
                },
                _ => continue,
            };
            if !ok {
                return; // remain degraded; try again later
            }
            files_to_remove.insert(file);
        }
        for f in files_to_remove {
            self.spool.remove(&f);
        }
    }
}
