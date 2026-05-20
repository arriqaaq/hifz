//! Health and livez probes.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::embed::Embedder;

/// Build the `/api/v1/health` JSON response.
pub async fn report(
    db: &Surreal<Db>,
    embedder: &Embedder,
    started_at: Instant,
    ollama_enabled: bool,
    git_path: Option<&PathBuf>,
) -> Result<serde_json::Value> {
    let uptime = started_at.elapsed().as_secs();

    let sessions = count(db, "SELECT count() AS c FROM session GROUP ALL").await;
    let runs = count(db, "SELECT count() AS c FROM run GROUP ALL").await;
    let observations = count(db, "SELECT count() AS c FROM observation GROUP ALL").await;
    let memories = count(db, "SELECT count() AS c FROM memory GROUP ALL").await;
    let commits = count(
        db,
        "SELECT count() AS c FROM observation WHERE obs_type = 'commit_made' GROUP ALL",
    )
    .await;

    // Report the active mode so adapters and the website can
    // surface it. Two-flag derivation:
    //   - `ollama_enabled` reflects whether the OllamaClient was constructed
    //     (i.e. OLLAMA_URL is set in config). Reachability is not probed
    //     here to keep the health endpoint fast.
    //   - `llm_evolve` (HIFZ_LLM_EVOLVE) gates whether `enrich::save_enriched`
    //     actually invokes the LLM at insert time.
    let llm_evolve = std::env::var("HIFZ_LLM_EVOLVE")
        .map(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(false);
    let mode = if ollama_enabled && llm_evolve {
        "llm_augmented"
    } else {
        "deterministic"
    };

    Ok(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "sessions": sessions,
        "runs": runs,
        "observations": observations,
        "memories": memories,
        "commits": commits,
        "uptime_seconds": uptime,
        "embedding_provider": "fastembed",
        "embedding_dimensions": embedder.dimension(),
        "ollama": ollama_enabled,
        "llm_evolve": llm_evolve,
        "mode": mode,
        "git_available": git_path.is_some(),
        "git_path": git_path.map(|p| p.display().to_string()),
    }))
}

async fn count(db: &Surreal<Db>, sql: &str) -> i64 {
    db.query(sql)
        .await
        .ok()
        .and_then(|mut r| r.take::<Vec<serde_json::Value>>(0).ok())
        .and_then(|v| v.first().and_then(|r| r.get("c").and_then(|c| c.as_i64())))
        .unwrap_or(0)
}
