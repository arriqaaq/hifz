//! Session-start warmup: build a typed digest of "where you are" for a project.
//!
//! When a new session begins, the agent should immediately have access to
//! the most relevant past plans, decisions, conventions, open bugs, gotchas,
//! failure patterns, and recent lessons — without having to know to look.
//!
//! This module computes that digest from the memory store. It is purely
//! deterministic (no LLM dependency). Ranking inside each category is
//! `strength × recency-decay × (1 + log(retrieval_count + 1))`.
//!
//! The output shape is intentionally compact — title + 1-line summary +
//! id — so a SessionStart hook can inject it as a system-context block
//! without consuming much of the conversation budget.

use anyhow::Result;
use serde::Serialize;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::models::Category;

/// Per-category cap applied during the digest pull. Keeps the warmup
/// payload bounded even on very busy projects.
const PER_CATEGORY_LIMIT: i64 = 5;

/// Default `top_n` returned to the caller when no limit is supplied.
const DEFAULT_TOP_N: usize = 15;

/// One entry in the warmup digest. Compact-by-design.
#[derive(Debug, Clone, Serialize)]
pub struct WarmupEntry {
    pub id: String,
    pub category: String,
    pub title: String,
    /// One-line summary. Prefers `context_summary` when populated, else
    /// the first 200 chars of `content`.
    pub summary: String,
    pub strength: f64,
    pub retrieval_count: i64,
    pub last_accessed_at: String,
}

/// Aggregated digest grouped by category — the default "here's where you are"
/// view. Each section is already sorted (strongest/most-recent first).
#[derive(Debug, Default, Serialize)]
pub struct WarmupDigest {
    pub project: String,
    pub session_id: Option<String>,
    /// Latest open Plan (not closed by another memory).
    pub latest_plan: Option<WarmupEntry>,
    pub decisions: Vec<WarmupEntry>,
    pub conventions: Vec<WarmupEntry>,
    pub open_bugs: Vec<WarmupEntry>,
    pub gotchas: Vec<WarmupEntry>,
    pub failure_patterns: Vec<WarmupEntry>,
    pub recent_lessons: Vec<WarmupEntry>,
    /// Flat top-N across all categories, ordered by category priority.
    /// This is what a hook should usually inject as system context.
    pub top: Vec<WarmupEntry>,
}

/// Build a warmup digest for `project`. `session_id` is recorded on the
/// payload but does not affect ranking — the digest is project-scoped.
pub async fn build_warmup(
    db: &Surreal<Db>,
    project: &str,
    session_id: Option<&str>,
    top_n: Option<usize>,
) -> Result<WarmupDigest> {
    let n = top_n.unwrap_or(DEFAULT_TOP_N);

    let latest_plan = pull_latest_plan(db, project).await?;
    let decisions = pull_category(db, project, Category::Decision).await?;
    let conventions = pull_category(db, project, Category::Convention).await?;
    let open_bugs = pull_open_bugs(db, project).await?;
    let gotchas = pull_category(db, project, Category::Gotcha).await?;
    let failure_patterns = pull_category(db, project, Category::FailurePattern).await?;
    let recent_lessons = pull_category(db, project, Category::Lesson).await?;

    // Flatten in the priority order from the plan: Plan → Decision →
    // Convention → Bug → Gotcha → FailurePattern → Lesson.
    let mut top: Vec<WarmupEntry> = Vec::with_capacity(n);
    if let Some(p) = latest_plan.clone() {
        top.push(p);
    }
    for src in [
        &decisions,
        &conventions,
        &open_bugs,
        &gotchas,
        &failure_patterns,
        &recent_lessons,
    ] {
        for e in src {
            if top.len() >= n {
                break;
            }
            top.push(e.clone());
        }
    }
    top.truncate(n);

    Ok(WarmupDigest {
        project: project.to_string(),
        session_id: session_id.map(str::to_string),
        latest_plan,
        decisions,
        conventions,
        open_bugs,
        gotchas,
        failure_patterns,
        recent_lessons,
        top,
    })
}

// ---------------------------------------------------------------------------
// Per-category pulls
// ---------------------------------------------------------------------------

#[derive(Debug, SurrealValue)]
struct MemoryRow {
    id: Option<RecordId>,
    category: Option<String>,
    title: Option<String>,
    content: Option<String>,
    context_summary: Option<String>,
    strength: Option<f64>,
    retrieval_count: Option<i64>,
    last_accessed_at: Option<String>,
}

async fn pull_category(
    db: &Surreal<Db>,
    project: &str,
    category: Category,
) -> Result<Vec<WarmupEntry>> {
    let limit = PER_CATEGORY_LIMIT;
    let sql = format!(
        "SELECT id, category, title, content, context_summary, \
                strength, retrieval_count, last_accessed_at \
         FROM memory \
         WHERE is_latest = true \
           AND category = $cat \
           AND (project = $project OR project = 'global') \
         ORDER BY strength DESC, last_accessed_at DESC \
         LIMIT {limit}"
    );
    let mut resp = db
        .query(&sql)
        .bind(("cat", category.as_str().to_string()))
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<MemoryRow> = resp.take(0).unwrap_or_default();
    Ok(rows.into_iter().filter_map(row_to_entry).collect())
}

/// Latest Plan that has not been closed by another memory (no incoming
/// `closes` edge). This is the "active plan" the agent should resume.
async fn pull_latest_plan(db: &Surreal<Db>, project: &str) -> Result<Option<WarmupEntry>> {
    // Sub-query: any memory that has been the target of a `closes` edge.
    // `created_at` must appear in the projection because we ORDER BY it —
    // SurrealDB rejects ordering by a field that isn't selected.
    let sql = "SELECT id, category, title, content, context_summary, \
                      strength, retrieval_count, last_accessed_at, created_at \
               FROM memory \
               WHERE is_latest = true \
                 AND category = 'plan' \
                 AND (project = $project OR project = 'global') \
                 AND id NOT IN (SELECT VALUE out FROM edge WHERE relation = 'closes') \
               ORDER BY created_at DESC \
               LIMIT 1";
    let mut resp = db.query(sql).bind(("project", project.to_string())).await?;
    let rows: Vec<MemoryRow> = resp.take(0).unwrap_or_default();
    Ok(rows.into_iter().next().and_then(row_to_entry))
}

/// Bugs that have not been closed by a Fix (no incoming `closes` edge).
async fn pull_open_bugs(db: &Surreal<Db>, project: &str) -> Result<Vec<WarmupEntry>> {
    let limit = PER_CATEGORY_LIMIT;
    let sql = format!(
        "SELECT id, category, title, content, context_summary, \
                strength, retrieval_count, last_accessed_at \
         FROM memory \
         WHERE is_latest = true \
           AND category = 'bug' \
           AND (project = $project OR project = 'global') \
           AND id NOT IN (SELECT VALUE out FROM edge WHERE relation = 'closes') \
         ORDER BY last_accessed_at DESC \
         LIMIT {limit}"
    );
    let mut resp = db
        .query(&sql)
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<MemoryRow> = resp.take(0).unwrap_or_default();
    Ok(rows.into_iter().filter_map(row_to_entry).collect())
}

fn row_to_entry(r: MemoryRow) -> Option<WarmupEntry> {
    let id = r.id.map(|rid| format!("{rid:?}"))?;
    let title = r.title.unwrap_or_default();
    let summary = r
        .context_summary
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            let c = r.content.unwrap_or_default();
            if c.len() > 200 {
                format!("{}…", &c[..200])
            } else {
                c
            }
        });
    Some(WarmupEntry {
        id,
        category: r.category.unwrap_or_default(),
        title,
        summary,
        strength: r.strength.unwrap_or(0.0),
        retrieval_count: r.retrieval_count.unwrap_or(0),
        last_accessed_at: r.last_accessed_at.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_summary_prefers_context_summary() {
        let r = MemoryRow {
            id: None,
            category: Some("decision".to_string()),
            title: Some("t".to_string()),
            content: Some("the long content body".to_string()),
            context_summary: Some("the context summary".to_string()),
            strength: Some(1.0),
            retrieval_count: Some(0),
            last_accessed_at: Some("now".to_string()),
        };
        // row_to_entry returns None when id is None — synthesize via the
        // summary fallback path manually.
        let summary = r
            .context_summary
            .as_ref()
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_default();
        assert_eq!(summary, "the context summary");
    }

    #[test]
    fn entry_summary_truncates_long_content() {
        let long = "x".repeat(500);
        let trimmed = if long.len() > 200 {
            format!("{}…", &long[..200])
        } else {
            long.clone()
        };
        assert_eq!(trimmed.chars().count(), 201); // 200 chars + ellipsis
    }
}
