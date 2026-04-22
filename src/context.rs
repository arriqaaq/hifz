use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::core_mem;
use crate::db::Db;
use crate::embed::Embedder;
use crate::rank;
use crate::run;
use crate::search;

/// Generate context for a new session or compaction boundary.
///
/// When `query` is provided (e.g. the user's prompt), retrieval is query-aware:
/// hybrid search finds relevant memories, then Rust-side scoring re-ranks by
/// `strength · exp(-age/30) · access_boost`, and a simple MMR-lite dedup drops
/// redundant entries (same mem_type + leading concept).
///
/// When no `query` is supplied, the function synthesises one from the project
/// name plus titles of the recent high-importance observations, so SessionStart
/// still gets topically relevant context rather than a blind top-N-by-strength
/// dump.
pub async fn generate_context(
    db: &Surreal<Db>,
    project: &str,
    token_budget: usize,
) -> Result<String> {
    generate_context_with_query(db, None, project, None, token_budget).await
}

/// Query-aware variant. `embedder` is required when `query` is provided.
pub async fn generate_context_with_query(
    db: &Surreal<Db>,
    embedder: Option<&Embedder>,
    project: &str,
    query: Option<&str>,
    token_budget: usize,
) -> Result<String> {
    let mut context = String::new();
    let mut tokens_used = 0;

    // 0. Core memory — always prepended, per-project.
    if let Ok(core_row) = core_mem::get(db, project).await {
        let rendered = core_mem::render(&core_row);
        if !rendered.is_empty() {
            let est = rendered.len() / 4;
            if est <= token_budget {
                context.push_str(&rendered);
                tokens_used += est;
            }
        }
    }

    // Build an effective query: use the supplied one, else synthesise from recent observations.
    let synthesised;
    let effective_query: Option<&str> = match query {
        Some(q) if !q.trim().is_empty() => Some(q),
        _ => {
            synthesised = synthesise_query(db, project).await.unwrap_or_default();
            if synthesised.is_empty() {
                None
            } else {
                Some(synthesised.as_str())
            }
        }
    };

    // 1. Saved memories.
    let memory_entries = if let (Some(q), Some(e)) = (effective_query, embedder) {
        query_aware_memories(db, e, project, q, 20).await?
    } else {
        top_memories_by_rank(db, project, 20).await?
    };

    let diversified = mmr_lite(memory_entries, 10);

    if !diversified.is_empty() {
        context.push_str("# Saved memories\n\n");
        for m in &diversified {
            let entry = format!(
                "- [{mtype}] **{title}**: {content}\n",
                mtype = m.mem_type,
                title = m.title,
                content = m.content
            );
            let est_tokens = entry.len() / 4;
            if tokens_used + est_tokens > token_budget {
                break;
            }
            context.push_str(&entry);
            tokens_used += est_tokens;
        }
        context.push('\n');
    }

    // 2. Consolidated semantic facts (from tier_semantic).
    let mut sem_resp = db
        .query(
            "SELECT fact, confidence FROM semantic_hifz \
             WHERE strength > 0.3 \
             ORDER BY confidence DESC LIMIT 10",
        )
        .await
        .ok();
    let semantic_facts: Vec<serde_json::Value> = sem_resp
        .as_mut()
        .and_then(|r| r.take(0).ok())
        .unwrap_or_default();

    if !semantic_facts.is_empty() && tokens_used < token_budget {
        context.push_str("# Known facts\n\n");
        for f in &semantic_facts {
            let fact = f.get("fact").and_then(|v| v.as_str()).unwrap_or("");
            if fact.is_empty() {
                continue;
            }
            let entry = format!("- {fact}\n");
            let est_tokens = entry.len() / 4;
            if tokens_used + est_tokens > token_budget {
                break;
            }
            context.push_str(&entry);
            tokens_used += est_tokens;
        }
        context.push('\n');
    }

    // 3. Consolidated procedural knowledge (from tier_procedural).
    let mut proc_resp = db
        .query(
            "SELECT name, steps, trigger_condition, frequency FROM procedural_hifz \
             WHERE strength > 0.3 \
             ORDER BY frequency DESC LIMIT 5",
        )
        .await
        .ok();
    let procedures: Vec<serde_json::Value> = proc_resp
        .as_mut()
        .and_then(|r| r.take(0).ok())
        .unwrap_or_default();

    if !procedures.is_empty() && tokens_used < token_budget {
        context.push_str("# Procedures\n\n");
        for p in &procedures {
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let trigger = p
                .get("trigger_condition")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let steps: Vec<&str> = p
                .get("steps")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|s| s.as_str()).collect())
                .unwrap_or_default();
            if name.is_empty() || steps.is_empty() {
                continue;
            }
            let entry = format!("- **{name}** (when: {trigger}): {}\n", steps.join(" -> "));
            let est_tokens = entry.len() / 4;
            if tokens_used + est_tokens > token_budget {
                break;
            }
            context.push_str(&entry);
            tokens_used += est_tokens;
        }
        context.push('\n');
    }

    // 4. Recent high-importance observations for this project.
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

    // 5. Recent task outcomes from closed runs (lessons learned).
    let runs = run::recent_with_lessons(db, project, effective_query, 8)
        .await
        .unwrap_or_default();
    if !runs.is_empty() && tokens_used < token_budget {
        context.push_str("\n# Recent task outcomes\n\n");
        for r in &runs {
            let prompt_preview = r
                .prompt
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(60)
                .collect::<String>();
            let lesson = r.lesson.as_deref().unwrap_or("");
            let outcome = r.outcome.as_deref().unwrap_or("success");
            let status_icon = match outcome {
                "committed" => "✓",
                "uncommitted" => "○",
                _ => "•",
            };
            let entry = format!("- {status_icon} **{prompt_preview}...**: {lesson}\n");
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

// --- Memory retrieval paths ---

struct MemoryEntry {
    title: String,
    content: String,
    mem_type: String,
    first_concept: Option<String>,
    score: f64,
}

#[derive(Debug, SurrealValue)]
struct MemRow {
    title: Option<String>,
    content: Option<String>,
    mem_type: Option<String>,
    concepts: Option<Vec<String>>,
    strength: Option<f64>,
    created_at: Option<String>,
    access_count: Option<i64>,
}

async fn top_memories_by_rank(
    db: &Surreal<Db>,
    project: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>> {
    let mut resp = db
        .query(
            "SELECT title, content, mem_type, concepts, strength, created_at, access_count \
             FROM hifz \
             WHERE is_latest = true AND (project = $project OR project = 'global') \
             LIMIT 100",
        )
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<MemRow> = resp.take(0)?;

    let mut entries: Vec<MemoryEntry> = rows
        .into_iter()
        .map(|m| {
            let base = m.strength.unwrap_or(1.0);
            let created = m.created_at.clone().unwrap_or_default();
            let access = m.access_count.unwrap_or(0);
            MemoryEntry {
                title: m.title.unwrap_or_default(),
                content: m.content.unwrap_or_default(),
                mem_type: m.mem_type.unwrap_or_default(),
                first_concept: m.concepts.and_then(|c| c.into_iter().next()),
                score: rank::final_score(base, &created, access),
            }
        })
        .collect();

    entries.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entries.truncate(limit);
    Ok(entries)
}

async fn query_aware_memories(
    db: &Surreal<Db>,
    embedder: &Embedder,
    project: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>> {
    let results = search::search_hybrid(db, embedder, query, limit, Some(project)).await?;
    let mem_ids: Vec<surrealdb::types::RecordId> = results
        .iter()
        .filter(|r| r.obs_type.starts_with("memory:"))
        .filter_map(|r| r.id.clone())
        .collect();
    if mem_ids.is_empty() {
        return Ok(vec![]);
    }

    let mut resp = db
        .query(
            "SELECT id, title, content, mem_type, concepts, strength, created_at, access_count \
             FROM hifz WHERE id IN $ids",
        )
        .bind(("ids", mem_ids.clone()))
        .await?;
    #[derive(Debug, SurrealValue)]
    struct RowWithId {
        id: Option<surrealdb::types::RecordId>,
        title: Option<String>,
        content: Option<String>,
        mem_type: Option<String>,
        concepts: Option<Vec<String>>,
        strength: Option<f64>,
        created_at: Option<String>,
        access_count: Option<i64>,
    }
    let rows: Vec<RowWithId> = resp.take(0)?;

    // Preserve hybrid-search rank order but carry Rust-side score for the dedup pass.
    let mut by_id: std::collections::HashMap<String, RowWithId> = rows
        .into_iter()
        .filter_map(|r| r.id.clone().map(|id| (format!("{id:?}"), r)))
        .collect();

    let mut entries = Vec::new();
    for id in &mem_ids {
        let key = format!("{id:?}");
        if let Some(r) = by_id.remove(&key) {
            let base = r.strength.unwrap_or(1.0);
            let created = r.created_at.clone().unwrap_or_default();
            let access = r.access_count.unwrap_or(0);
            entries.push(MemoryEntry {
                title: r.title.unwrap_or_default(),
                content: r.content.unwrap_or_default(),
                mem_type: r.mem_type.unwrap_or_default(),
                first_concept: r.concepts.and_then(|c| c.into_iter().next()),
                score: rank::final_score(base, &created, access),
            });
        }
    }
    Ok(entries)
}

/// Synthesise a query from project state when no explicit query is given.
/// Uses project name + titles of the most recent high-importance observations.
async fn synthesise_query(db: &Surreal<Db>, project: &str) -> Result<String> {
    let mut resp = db
        .query(
            "SELECT VALUE title FROM observation \
             WHERE session_id.project = $project AND importance >= 5 \
             ORDER BY importance DESC LIMIT 5",
        )
        .bind(("project", project.to_string()))
        .await?;
    let titles: Vec<String> = resp.take(0).unwrap_or_default();
    let mut q = project.to_string();
    if !titles.is_empty() {
        q.push(' ');
        q.push_str(&titles.join(" "));
    }
    Ok(q)
}

/// MMR-lite: dedup by (mem_type, first concept) so we don't return 10 variants
/// of the same pattern. Cheap, deterministic, no embedding round-trip required.
/// Phase-3 graph expansion and Phase-1c proper cosine-based MMR will build on this.
fn mmr_lite(entries: Vec<MemoryEntry>, limit: usize) -> Vec<MemoryEntry> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in entries {
        let key = (
            e.mem_type.clone(),
            e.first_concept.clone().unwrap_or_default(),
        );
        if seen.insert(key) {
            out.push(e);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}
