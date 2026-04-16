use anyhow::Result;
use std::collections::HashMap;
use surrealdb::Surreal;

use crate::db::Db;
use crate::models::ProjectDigest;

/// Generate a project digest with top concepts, files, and stats.
pub async fn generate_digest(db: &Surreal<Db>, project: &str) -> Result<ProjectDigest> {
    let now = chrono::Utc::now().to_rfc3339();

    // Count sessions
    let mut resp = db
        .query("SELECT count() AS c FROM session WHERE project = $project GROUP ALL")
        .bind(("project", project.to_string()))
        .await?;
    let session_count: i64 = resp
        .take::<Vec<serde_json::Value>>(0)?
        .first()
        .and_then(|v| v.get("c"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Count observations
    let mut resp = db
        .query(
            "SELECT count() AS c FROM observation \
             WHERE session_id.project = $project GROUP ALL",
        )
        .bind(("project", project.to_string()))
        .await?;
    let total_observations: i64 = resp
        .take::<Vec<serde_json::Value>>(0)?
        .first()
        .and_then(|v| v.get("c"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Get concept frequencies
    let mut resp = db
        .query(
            "SELECT concepts FROM observation \
             WHERE session_id.project = $project \
             ORDER BY timestamp DESC LIMIT 200",
        )
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;

    let mut concept_freq: HashMap<String, i64> = HashMap::new();
    let mut file_freq: HashMap<String, i64> = HashMap::new();

    for row in &rows {
        if let Some(concepts) = row.get("concepts").and_then(|v| v.as_array()) {
            for c in concepts {
                if let Some(s) = c.as_str() {
                    *concept_freq.entry(s.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    // Get file frequencies
    let mut resp = db
        .query(
            "SELECT files FROM observation \
             WHERE session_id.project = $project \
             ORDER BY timestamp DESC LIMIT 200",
        )
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;
    for row in &rows {
        if let Some(files) = row.get("files").and_then(|v| v.as_array()) {
            for f in files {
                if let Some(s) = f.as_str() {
                    *file_freq.entry(s.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    let mut top_concepts: Vec<_> = concept_freq
        .into_iter()
        .map(|(concept, frequency)| crate::models::ConceptFreq { concept, frequency })
        .collect();
    top_concepts.sort_by(|a, b| b.frequency.cmp(&a.frequency));
    top_concepts.truncate(20);

    let mut top_files: Vec<_> = file_freq
        .into_iter()
        .map(|(file, frequency)| crate::models::FileFreq { file, frequency })
        .collect();
    top_files.sort_by(|a, b| b.frequency.cmp(&a.frequency));
    top_files.truncate(20);

    Ok(ProjectDigest {
        project: project.to_string(),
        updated_at: now,
        top_concepts,
        top_files,
        session_count,
        total_observations,
    })
}
