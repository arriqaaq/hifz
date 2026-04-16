use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;

/// Generate context for a new session based on project history + saved memories.
pub async fn generate_context(
    db: &Surreal<Db>,
    project: &str,
    token_budget: usize,
) -> Result<String> {
    let mut context = String::new();
    let mut tokens_used = 0;

    // 1. Include saved memories (hifz table) — highest priority
    let mut mem_resp = db
        .query(
            "SELECT title, content, mem_type, strength \
             FROM hifz WHERE is_latest = true \
             ORDER BY strength DESC \
             LIMIT 10",
        )
        .await?;
    let memories: Vec<serde_json::Value> = mem_resp.take(0)?;

    if !memories.is_empty() {
        context.push_str("# Saved memories\n\n");
        for m in &memories {
            let title = m.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let content = m.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let mem_type = m.get("mem_type").and_then(|v| v.as_str()).unwrap_or("");
            let entry = format!("- [{mem_type}] **{title}**: {content}\n");
            let est_tokens = entry.len() / 4;
            if tokens_used + est_tokens > token_budget {
                break;
            }
            context.push_str(&entry);
            tokens_used += est_tokens;
        }
        context.push('\n');
    }

    // 2. Include recent high-importance observations
    let mut obs_resp = db
        .query(
            "SELECT title, narrative, obs_type, importance, timestamp \
             FROM observation \
             WHERE session_id.project = $project AND importance >= 4 \
             ORDER BY timestamp DESC \
             LIMIT 20",
        )
        .bind(("project", project.to_string()))
        .await?;
    let obs: Vec<serde_json::Value> = obs_resp.take(0)?;

    if !obs.is_empty() && tokens_used < token_budget {
        context.push_str("# Recent observations\n\n");
        for o in &obs {
            let title = o.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let narrative = o.get("narrative").and_then(|v| v.as_str()).unwrap_or("");
            let entry = format!("- **{title}**: {narrative}\n");
            let est_tokens = entry.len() / 4;
            if tokens_used + est_tokens > token_budget {
                break;
            }
            context.push_str(&entry);
            tokens_used += est_tokens;
        }
    }

    Ok(context)
}
