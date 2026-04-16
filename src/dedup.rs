use dashmap::DashMap;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

const TTL: Duration = Duration::from_secs(300); // 5 minutes
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Content-based deduplication map with TTL expiry.
pub struct DedupMap {
    entries: Arc<DashMap<String, Instant>>,
    cleanup_handle: Option<JoinHandle<()>>,
}

impl DedupMap {
    pub fn new() -> Self {
        let entries = Arc::new(DashMap::new());
        let entries_clone = entries.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
            loop {
                interval.tick().await;
                let now = Instant::now();
                entries_clone.retain(|_, expires_at| *expires_at > now);
            }
        });

        Self {
            entries,
            cleanup_handle: Some(handle),
        }
    }

    /// Compute a SHA-256 fingerprint for deduplication.
    pub fn compute_hash(session_id: &str, tool_name: &str, tool_input: &str) -> String {
        let input = if tool_input.len() > 500 {
            &tool_input[..500]
        } else {
            tool_input
        };
        let raw = format!("{session_id}:{tool_name}:{input}");
        let hash = Sha256::digest(raw.as_bytes());
        hex::encode(hash)
    }

    /// Check if this hash was seen recently.
    pub fn is_duplicate(&self, hash: &str) -> bool {
        if let Some(entry) = self.entries.get(hash) {
            *entry > Instant::now()
        } else {
            false
        }
    }

    /// Record a hash with TTL.
    pub fn record(&self, hash: String) {
        self.entries.insert(hash, Instant::now() + TTL);
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.cleanup_handle.take() {
            handle.abort();
        }
    }
}

impl Drop for DedupMap {
    fn drop(&mut self) {
        self.stop();
    }
}
