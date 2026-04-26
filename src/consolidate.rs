use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::ollama::OllamaClient;
use crate::prompts;

/// Run the 4-tier consolidation pipeline.
/// Tiers that require LLM (semantic merge, procedural extraction) are skipped if Ollama is not available.
pub async fn run_consolidation(
    db: &Surreal<Db>,
    ollama: Option<&OllamaClient>,
) -> Result<ConsolidationResult> {
    let mut result = ConsolidationResult::default();

    // Tier 1: Semantic — merge session summaries into facts (requires LLM)
    if let Some(ollama) = ollama {
        let semantic_count = tier_semantic(db, ollama).await.unwrap_or(0);
        result.semantic_facts_created = semantic_count;
    }

    // Tier 2: Reflect — cluster related memories (no LLM needed)
    // This is a simpler version that groups by shared keywords
    result.clusters_created = tier_reflect(db).await.unwrap_or(0);

    // Tier 3: Procedural — extract workflows (requires LLM)
    if let Some(ollama) = ollama {
        let proc_count = tier_procedural(db, ollama).await.unwrap_or(0);
        result.procedures_extracted = proc_count;
    }

    // Tier 4: Decay — apply exponential decay to old memories
    result.decayed = tier_decay(db, 30).await.unwrap_or(0);

    Ok(result)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ConsolidationResult {
    pub semantic_facts_created: usize,
    pub clusters_created: usize,
    pub procedures_extracted: usize,
    pub decayed: usize,
}

async fn tier_semantic(db: &Surreal<Db>, ollama: &OllamaClient) -> Result<usize> {
    let mut resp = db
        .query("SELECT * FROM summary ORDER BY created_at DESC LIMIT 20")
        .await?;
    let summaries: Vec<serde_json::Value> = resp.take(0)?;

    if summaries.len() < 5 {
        return Ok(0);
    }

    let summaries_text = serde_json::to_string_pretty(&summaries)?;
    let response = ollama
        .complete(prompts::SEMANTIC_MERGE_SYSTEM, &summaries_text)
        .await?;

    // Parse <fact confidence="X">content</fact> entries
    let fact_re = regex::Regex::new(r#"<fact\s+confidence="([^"]+)">([^<]+)</fact>"#)?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut count = 0;

    for cap in fact_re.captures_iter(&response) {
        let confidence: f64 = cap[1].parse().unwrap_or(0.5);
        let fact = cap[2].trim().to_string();
        if fact.is_empty() {
            continue;
        }

        db.query(
            "CREATE semantic_memory SET \
             fact = $fact, confidence = $confidence, \
             retrieval_count = 0, strength = 1.0, \
             last_accessed_at = $now, created_at = $now, updated_at = $now",
        )
        .bind(("fact", fact))
        .bind(("confidence", confidence))
        .bind(("now", now.clone()))
        .await?;
        count += 1;
    }

    Ok(count)
}

async fn tier_reflect(db: &Surreal<Db>) -> Result<usize> {
    use surrealdb::types::{RecordId, SurrealValue};

    #[derive(Debug, SurrealValue)]
    struct MemRow {
        id: Option<RecordId>,
        keywords: Option<Vec<String>>,
    }

    let mut resp = db
        .query("SELECT id, keywords FROM memory WHERE is_latest = true LIMIT 100")
        .await?;
    let rows: Vec<MemRow> = resp.take(0)?;

    if rows.len() < 3 {
        return Ok(0);
    }

    // Filter stop-keywords: remove any keyword appearing in >50% of memories
    let mut freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for row in &rows {
        if let Some(ref keywords) = row.keywords {
            for c in keywords {
                *freq.entry(c.clone()).or_default() += 1;
            }
        }
    }
    let threshold = rows.len() / 2;
    let stop_keywords: std::collections::HashSet<&str> = freq
        .iter()
        .filter(|(_, count)| **count > threshold)
        .map(|(c, _)| c.as_str())
        .collect();

    // Build filtered keyword sets per memory
    let filtered: Vec<(Option<&RecordId>, std::collections::HashSet<String>)> = rows
        .iter()
        .map(|r| {
            let keywords: std::collections::HashSet<String> = r
                .keywords
                .as_ref()
                .map(|cs| {
                    cs.iter()
                        .filter(|c| !stop_keywords.contains(c.as_str()))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (r.id.as_ref(), keywords)
        })
        .collect();

    // Pairwise Jaccard, collect clusters
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut clustered: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for i in 0..filtered.len() {
        if clustered.contains(&i) || filtered[i].1.is_empty() {
            continue;
        }
        let mut cluster = vec![i];
        for j in (i + 1)..filtered.len() {
            if clustered.contains(&j) || filtered[j].1.is_empty() {
                continue;
            }
            let intersection = filtered[i].1.intersection(&filtered[j].1).count();
            let union = filtered[i].1.union(&filtered[j].1).count();
            if union > 0 && (intersection as f64 / union as f64) >= 0.4 {
                cluster.push(j);
            }
        }
        if cluster.len() >= 3 {
            for &idx in &cluster {
                clustered.insert(idx);
            }
            clusters.push(cluster);
        }
    }

    let mut count = 0;
    for cluster in &clusters {
        for i in 0..cluster.len() {
            for j in (i + 1)..cluster.len() {
                let from = filtered[cluster[i]].0;
                let to = filtered[cluster[j]].0;
                if let (Some(from_id), Some(to_id)) = (from, to) {
                    let _ =
                        crate::link::upsert_edge(db, from_id, to_id, "similar_to", "cluster", 0.5)
                            .await;
                }
            }
        }
        count += 1;
    }

    Ok(count)
}

async fn tier_procedural(db: &Surreal<Db>, ollama: &OllamaClient) -> Result<usize> {
    let mut resp = db
        .query(
            "SELECT title, content, strength FROM memory \
             WHERE is_latest = true AND category = 'pattern' \
             ORDER BY strength DESC LIMIT 20",
        )
        .await?;
    let patterns: Vec<serde_json::Value> = resp.take(0)?;

    if patterns.len() < 2 {
        return Ok(0);
    }

    let patterns_text = serde_json::to_string_pretty(&patterns)?;
    let response = ollama
        .complete(prompts::PROCEDURAL_EXTRACTION_SYSTEM, &patterns_text)
        .await?;

    let proc_re = regex::Regex::new(
        r#"<procedure\s+name="([^"]+)"\s+trigger="([^"]+)">([\s\S]*?)</procedure>"#,
    )?;
    let step_re = regex::Regex::new(r"<step>([^<]+)</step>")?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut count = 0;

    for cap in proc_re.captures_iter(&response) {
        let name = cap[1].trim().to_string();
        let trigger = cap[2].trim().to_string();
        let body = &cap[3];

        let steps: Vec<String> = step_re
            .captures_iter(body)
            .map(|c| c[1].trim().to_string())
            .collect();

        if steps.is_empty() {
            continue;
        }

        db.query(
            "CREATE procedural_memory SET \
             name = $name, steps = $steps, trigger_condition = $trigger, \
             frequency = 1, strength = 1.0, \
             created_at = $now, updated_at = $now",
        )
        .bind(("name", name))
        .bind(("steps", steps))
        .bind(("trigger", trigger))
        .bind(("now", now.clone()))
        .await?;
        count += 1;
    }

    Ok(count)
}

async fn tier_decay(db: &Surreal<Db>, decay_days: i64) -> Result<usize> {
    // Apply exponential decay: strength *= 0.9 for each decay period elapsed
    let mut resp = db
        .query(
            "SELECT id, strength, last_accessed_at FROM semantic_memory \
             WHERE strength > 0.1",
        )
        .await?;
    let memories: Vec<serde_json::Value> = resp.take(0)?;

    let now = chrono::Utc::now();
    let mut decayed = 0;

    for mem in &memories {
        let last_accessed = mem
            .get("last_accessed_at")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        if let Some(last) = last_accessed {
            let days_since = (now - last).num_days();
            if days_since > decay_days {
                let periods = days_since / decay_days;
                let current_strength = mem.get("strength").and_then(|v| v.as_f64()).unwrap_or(1.0);
                let new_strength = (current_strength * 0.9_f64.powi(periods as i32)).max(0.1);

                if let Some(id) = mem.get("id").and_then(|v| v.as_str()) {
                    db.query("UPDATE type::record($id) SET strength = $strength")
                        .bind(("id", id.to_string()))
                        .bind(("strength", new_strength))
                        .await?;
                    decayed += 1;
                }
            }
        }
    }

    // Also decay memory table memories (longer period: 60 days)
    let memory_decay_days: i64 = 60;
    let mut resp = db
        .query(
            "SELECT id, strength, last_accessed_at FROM memory \
             WHERE strength > 0.1 AND is_latest = true",
        )
        .await?;
    let memory_memories: Vec<serde_json::Value> = resp.take(0)?;

    for mem in &memory_memories {
        let last_accessed = mem
            .get("last_accessed_at")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        if let Some(last) = last_accessed {
            let days_since = (now - last).num_days();
            if days_since > memory_decay_days {
                let periods = days_since / memory_decay_days;
                let current_strength = mem.get("strength").and_then(|v| v.as_f64()).unwrap_or(1.0);
                let new_strength = (current_strength * 0.9_f64.powi(periods as i32)).max(0.1);

                if (new_strength - current_strength).abs() > 0.001 {
                    if let Some(id) = mem.get("id").and_then(|v| v.as_str()) {
                        db.query("UPDATE type::record($id) SET strength = $strength")
                            .bind(("id", id.to_string()))
                            .bind(("strength", new_strength))
                            .await?;
                        decayed += 1;
                    }
                }
            }
        }
    }

    Ok(decayed)
}
