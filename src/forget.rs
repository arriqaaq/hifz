use anyhow::Result;
use std::collections::{HashMap, HashSet};
use surrealdb::Surreal;

use crate::db::Db;

/// Run forget GC: TTL expiry + contradiction detection + low-value eviction.
pub async fn run_forget(db: &Surreal<Db>, dry_run: bool) -> Result<ForgetResult> {
    let mut result = ForgetResult::default();

    // 1. TTL expiry
    let expired = find_expired_memories(db).await?;
    result.ttl_expired = expired.len();
    if !dry_run {
        for id in &expired {
            db.query("DELETE type::record($id)")
                .bind(("id", id.clone()))
                .await?;
        }
    }

    // 2. Contradiction detection via Jaccard similarity
    let contradictions = find_contradictions(db).await?;
    result.contradictions = contradictions.len();
    if !dry_run {
        for id in &contradictions {
            db.query("UPDATE type::record($id) SET is_latest = false")
                .bind(("id", id.clone()))
                .await?;
        }
    }

    // 3. Low-value observation cleanup (>180 days, importance ≤ 2)
    let low_value = find_low_value_observations(db).await?;
    result.low_value_cleaned = low_value.len();
    if !dry_run {
        for id in &low_value {
            db.query("DELETE type::record($id)")
                .bind(("id", id.clone()))
                .await?;
        }
    }

    // 4. Weak memory cleanup (strength < 0.1)
    let weak = find_weak_memories(db).await?;
    result.weak_memories_cleaned = weak.len();
    if !dry_run {
        for id in &weak {
            db.query("DELETE type::record($id)")
                .bind(("id", id.clone()))
                .await?;
        }
    }

    Ok(result)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ForgetResult {
    pub ttl_expired: usize,
    pub contradictions: usize,
    pub low_value_cleaned: usize,
    pub weak_memories_cleaned: usize,
}

async fn find_expired_memories(db: &Surreal<Db>) -> Result<Vec<String>> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut resp = db
        .query(
            "SELECT id FROM hifz \
             WHERE forget_after IS NOT NONE AND forget_after < $now",
        )
        .bind(("now", now.clone()))
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect())
}

async fn find_contradictions(db: &Surreal<Db>) -> Result<Vec<String>> {
    // Load recent memories and check Jaccard similarity
    let mut resp = db
        .query("SELECT id, title, content, concepts FROM hifz WHERE is_latest = true ORDER BY updated_at DESC LIMIT 100")
        .await?;
    let memories: Vec<serde_json::Value> = resp.take(0)?;

    let mut to_mark: Vec<String> = Vec::new();
    let threshold = 0.9;

    // Build concept index for efficient comparison
    let mut concept_index: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, mem) in memories.iter().enumerate() {
        if let Some(concepts) = mem.get("concepts").and_then(|v| v.as_array()) {
            for c in concepts {
                if let Some(s) = c.as_str() {
                    concept_index.entry(s.to_lowercase()).or_default().push(i);
                }
            }
        }
    }

    // Compare only memories sharing concepts
    let mut compared: HashSet<(usize, usize)> = HashSet::new();
    for indices in concept_index.values() {
        for i in 0..indices.len() {
            for j in (i + 1)..indices.len() {
                let (a, b) = (indices[i], indices[j]);
                if compared.contains(&(a, b)) {
                    continue;
                }
                compared.insert((a, b));

                let content_a = memories[a]
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content_b = memories[b]
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if jaccard_similarity(content_a, content_b) >= threshold {
                    // Keep newer (lower index = more recent), mark older
                    let older_id = memories[b].get("id").and_then(|v| v.as_str()).unwrap_or("");
                    if !older_id.is_empty() {
                        to_mark.push(older_id.to_string());
                    }
                }
            }
        }
    }

    Ok(to_mark)
}

fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let set_a: HashSet<&str> = a.split_whitespace().filter(|w| w.len() > 2).collect();
    let set_b: HashSet<&str> = b.split_whitespace().filter(|w| w.len() > 2).collect();

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        return 0.0;
    }
    intersection as f64 / union as f64
}

async fn find_weak_memories(db: &Surreal<Db>) -> Result<Vec<String>> {
    // Memories with strength decayed below 0.1
    let mut resp = db
        .query("SELECT id FROM hifz WHERE strength < 0.1 LIMIT 100")
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect())
}

async fn find_low_value_observations(db: &Surreal<Db>) -> Result<Vec<String>> {
    // Observations older than 180 days with importance ≤ 2
    let mut resp = db
        .query(
            "SELECT id FROM observation \
             WHERE importance <= 2 \
             AND time::unix(timestamp) < time::unix(time::now()) - 15552000 \
             LIMIT 100",
        )
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;
    Ok(rows
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect())
}
