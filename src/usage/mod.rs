//! Generic agent-usage tracking.
//!
//! Adapters (e.g. `adapters/claude-code/`) post per-inference-call token
//! records via the REST endpoints; hifz core stores them in the
//! `agent_usage` table and aggregates them for session/project views.
//!
//! Everything in this module is vendor-neutral. Anthropic-specific concepts
//! (cache_read, cache_creation, JSONL parsing) live in the adapter — never
//! here.

pub mod aggregate;
pub mod patterns;

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::models::{AgentUsage, AgentUsageRecord, UsagePattern};

// `record<session>` references are stored under their bare key only — the
// `session:` prefix is implied by the field's record-type constraint. The
// `ensure_session_prefix` / `strip_session_prefix` helpers below normalize
// caller-supplied ids (which may or may not include the prefix) before we
// hand them to `RecordId::new`.

/// Result of an ingest call. `inserted` counts rows newly written; rows
/// already present (matched by `(agent, external_id) UNIQUE`) are no-ops
/// and counted in `skipped`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestResult {
    pub inserted: i64,
    pub skipped: i64,
}

/// Insert one record. Idempotent: relies on the `(agent, external_id)`
/// UNIQUE index — a duplicate insert is caught and counted as `skipped`
/// instead of erroring. Concurrent writers also see a `Transaction
/// conflict` from SurrealDB; we treat that the same way after re-checking
/// `exists()`.
pub async fn record(db: &Surreal<Db>, rec: AgentUsageRecord) -> Result<IngestResult> {
    let row = build_row(rec)?;
    insert_row(db, row).await
}

/// Bulk insert. Idempotent. Two-layer dedup:
///   1. In-memory `(agent, external_id)` set absorbs within-batch dupes
///      (the parser already dedupes per file, but a re-post can hand us
///      overlapping batches).
///   2. The DB `(agent, external_id)` UNIQUE catches cross-batch dupes;
///      `insert_row` retries once on transaction conflicts and counts the
///      losing writer as `skipped` rather than aborting the whole batch.
pub async fn record_batch(
    db: &Surreal<Db>,
    records: Vec<AgentUsageRecord>,
) -> Result<IngestResult> {
    let mut result = IngestResult::default();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for rec in records {
        let row = match build_row(rec) {
            Ok(r) => r,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        if !seen.insert((row.agent.clone(), row.external_id.clone())) {
            result.skipped += 1;
            continue;
        }
        let r = insert_row(db, row).await?;
        result.inserted += r.inserted;
        result.skipped += r.skipped;
    }
    Ok(result)
}

/// Single-row insert with idempotent semantics. Returns Ok in three cases:
///   - row created           → `inserted=1`
///   - row already present   → `skipped=1`
///   - DB returned a unique-violation or transaction-conflict error, and a
///     follow-up `exists()` confirms the row is now present (or a one-shot
///     retry also failed). → `skipped=1` plus a warning log.
///
/// Only non-recoverable errors (build_row / unrelated DB failure) are
/// returned via `Err` to the caller; per-row failures never abort a batch.
async fn insert_row(db: &Surreal<Db>, row: AgentUsageInsert) -> Result<IngestResult> {
    let attempt: Result<Option<AgentUsage>, surrealdb::Error> =
        db.create("agent_usage").content(row.clone()).await;
    match attempt {
        Ok(_) => Ok(IngestResult {
            inserted: 1,
            skipped: 0,
        }),
        Err(e) if is_unique_or_conflict(&e) => {
            // Either the row landed (UNIQUE caught us) or another writer
            // raced and committed first. Re-check; if still missing,
            // attempt one more write before giving up.
            if exists(db, &row.agent, &row.external_id)
                .await
                .unwrap_or(false)
            {
                return Ok(IngestResult {
                    inserted: 0,
                    skipped: 1,
                });
            }
            let retry: Result<Option<AgentUsage>, surrealdb::Error> =
                db.create("agent_usage").content(row.clone()).await;
            match retry {
                Ok(_) => Ok(IngestResult {
                    inserted: 1,
                    skipped: 0,
                }),
                Err(e2) => {
                    tracing::warn!(
                        agent = %row.agent,
                        external_id = %row.external_id,
                        error = %e2,
                        "agent_usage insert gave up after one retry"
                    );
                    Ok(IngestResult {
                        inserted: 0,
                        skipped: 1,
                    })
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                agent = %row.agent,
                external_id = %row.external_id,
                error = %e,
                "agent_usage insert failed (non-conflict)"
            );
            Ok(IngestResult {
                inserted: 0,
                skipped: 1,
            })
        }
    }
}

// SurrealDB doesn't surface typed errors for these two cases; string-match
// is the pragmatic test. Lower-cases the error so we tolerate phrasing
// drift between releases. Note the index-violation phrasing in this rev is
// `Database index `au_ext_uniq` already contains [...], with record ...` —
// which contains neither "unique" (only "uniq") nor "already exists", hence
// the explicit "already contains" / index-violation disjuncts below.
fn is_unique_or_conflict(e: &surrealdb::Error) -> bool {
    is_unique_or_conflict_msg(&e.to_string())
}

/// Message-level test, split out so it can be unit-tested without
/// constructing a `surrealdb::Error`.
fn is_unique_or_conflict_msg(raw: &str) -> bool {
    let msg = raw.to_lowercase();
    msg.contains("unique")
        || msg.contains("transaction conflict")
        || msg.contains("write conflict")
        || msg.contains("already exists")
        || msg.contains("already contains")
        || (msg.contains("database index") && msg.contains("contains"))
}

#[cfg(test)]
mod conflict_tests {
    use super::is_unique_or_conflict_msg;

    #[test]
    fn recognizes_surrealdb_unique_index_violation() {
        let msg = "Database index `au_ext_uniq` already contains \
                   ['claude-code', 'msg_01XfzQqX2zEPdWEf4Bbci532'], \
                   with record `agent_usage:rx8ls17u6o8eg2teyzey`";
        assert!(is_unique_or_conflict_msg(msg));
    }

    #[test]
    fn recognizes_classic_phrasings() {
        assert!(is_unique_or_conflict_msg("Unique constraint violation"));
        assert!(is_unique_or_conflict_msg("Transaction conflict"));
        assert!(is_unique_or_conflict_msg("write conflict detected"));
        assert!(is_unique_or_conflict_msg("record already exists"));
    }

    #[test]
    fn ignores_unrelated_errors() {
        assert!(!is_unique_or_conflict_msg("connection refused"));
        assert!(!is_unique_or_conflict_msg(
            "deserialization failed: missing field"
        ));
    }
}

/// Delete every usage row tied to a session. Provided for callers that want
/// to hard-delete a session (hifz doesn't ship that path today, but if it
/// ever does, drop a `delete_for_session` call into the same transaction
/// to keep `agent_usage` consistent).
pub async fn delete_for_session(db: &Surreal<Db>, session_id: &str) -> Result<i64> {
    let sid_key = strip_session_prefix(session_id).to_string();
    let mut resp = db
        .query("DELETE agent_usage WHERE record::id(session_id) = $sid RETURN BEFORE;")
        .bind(("sid", sid_key))
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Ok(rows.len() as i64)
}

/// Per-session view: aggregates + per-call array + session-scoped patterns.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionUsageView {
    pub session_id: String,
    pub totals: aggregate::TokenTotals,
    pub model: Option<String>,
    pub primary_agent: Option<String>,
    pub call_count: i64,
    pub calls: Vec<aggregate::UsageCallRow>,
    pub patterns: Vec<UsagePattern>,
}

pub async fn for_session(db: &Surreal<Db>, session_id: &str) -> Result<SessionUsageView> {
    let sid = ensure_session_prefix(session_id);
    let calls = aggregate::calls_for_session(db, &sid).await?;
    let totals = aggregate::sum_calls(&calls);
    let model = calls
        .iter()
        .max_by_key(|c| c.total_tokens)
        .map(|c| c.model.clone());
    let primary_agent = calls.first().map(|c| c.agent.clone());
    let patterns = patterns::for_session(&calls);
    Ok(SessionUsageView {
        session_id: strip_session_prefix(&sid).to_string(),
        call_count: calls.len() as i64,
        totals,
        model,
        primary_agent,
        calls,
        patterns,
    })
}

/// Filters for the per-project view.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProjectFilters {
    pub from: Option<String>,
    pub to: Option<String>,
    pub model: Option<String>,
}

/// Per-project view: the fat payload that drives the `/tokens` dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectUsageView {
    pub project: String,
    pub totals: aggregate::TokenTotals,
    pub session_count: i64,
    pub call_count: i64,
    pub date_range: Option<aggregate::DateRange>,
    pub daily: Vec<aggregate::DailyBucket>,
    pub models: Vec<aggregate::ModelBucket>,
    pub top_prompts: Vec<aggregate::PromptRow>,
    pub top_sessions: Vec<aggregate::SessionRow>,
    pub patterns: Vec<UsagePattern>,
}

pub async fn for_project(
    db: &Surreal<Db>,
    project: &str,
    filters: ProjectFilters,
) -> Result<ProjectUsageView> {
    let calls = aggregate::calls_for_project(db, project, &filters).await?;
    let totals = aggregate::sum_calls(&calls);
    let session_count = calls
        .iter()
        .map(|c| c.session_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .len() as i64;
    let date_range = aggregate::date_range(&calls);
    let daily = aggregate::daily_buckets(&calls);
    let models = aggregate::model_buckets(&calls);
    let top_prompts = aggregate::top_prompts(&calls, 20);
    let top_sessions = aggregate::top_sessions(&calls, 10);
    let patterns = patterns::for_project(&calls);
    Ok(ProjectUsageView {
        project: project.to_string(),
        totals,
        session_count,
        call_count: calls.len() as i64,
        date_range,
        daily,
        models,
        top_prompts,
        top_sessions,
        patterns,
    })
}

/// Per-session token totals, uncapped. Drives the `/sessions` list column
/// (which spans projects). `project=None` returns every session that has
/// usage data; `Some(p)` scopes to one project. Reuses `top_sessions`
/// with no cap so the row shape (session_id short-key, first_prompt,
/// total, calls, model, date) matches the dashboard's existing type.
pub async fn session_totals(
    db: &Surreal<Db>,
    project: Option<&str>,
) -> Result<Vec<aggregate::SessionRow>> {
    let calls = match project {
        Some(p) if !p.is_empty() => {
            aggregate::calls_for_project(db, p, &ProjectFilters::default()).await?
        }
        _ => aggregate::calls_all(db).await?,
    };
    Ok(aggregate::top_sessions(&calls, usize::MAX))
}

// --- internals ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct AgentUsageInsert {
    session_id: RecordId,
    project: String,
    agent: String,
    provider: Option<String>,
    model: String,
    external_id: String,
    timestamp: String,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    prompt: Option<String>,
    prompt_at: Option<String>,
    tools: Vec<String>,
    run_id: Option<RecordId>,
    breakdown: Option<serde_json::Value>,
    aux_calls: Option<i64>,
}

fn build_row(rec: AgentUsageRecord) -> Result<AgentUsageInsert> {
    if rec.external_id.is_empty() {
        return Err(anyhow!("external_id required"));
    }
    if rec.agent.is_empty() {
        return Err(anyhow!("agent required"));
    }
    let sid_key = strip_session_prefix(&rec.session_id).to_string();
    if sid_key.is_empty() {
        return Err(anyhow!("session_id required"));
    }
    let session_id = RecordId::new("session".to_string(), sid_key);
    let run_id = rec.run_id.as_ref().map(|r| {
        let key = r.strip_prefix("run:").unwrap_or(r).to_string();
        RecordId::new("run".to_string(), key)
    });

    let total = rec.total_tokens.unwrap_or_else(|| {
        let mut t = rec.input_tokens + rec.output_tokens;
        if let Some(serde_json::Value::Object(map)) = &rec.breakdown {
            for v in map.values() {
                if let Some(n) = v.as_i64() {
                    t += n;
                }
            }
        }
        t
    });

    Ok(AgentUsageInsert {
        session_id,
        project: rec.project,
        agent: rec.agent,
        provider: rec.provider,
        model: rec.model,
        external_id: rec.external_id,
        timestamp: rec.timestamp,
        input_tokens: rec.input_tokens,
        output_tokens: rec.output_tokens,
        total_tokens: total,
        prompt: rec.prompt,
        prompt_at: rec.prompt_at,
        tools: rec.tools,
        run_id,
        breakdown: rec.breakdown,
        aux_calls: rec.aux_calls,
    })
}

async fn exists(db: &Surreal<Db>, agent: &str, external_id: &str) -> Result<bool> {
    let mut resp = db
        .query("SELECT VALUE id FROM agent_usage WHERE agent = $a AND external_id = $e LIMIT 1;")
        .bind(("a", agent.to_string()))
        .bind(("e", external_id.to_string()))
        .await
        .context("dedup lookup")?;
    let hits: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    Ok(!hits.is_empty())
}

fn strip_session_prefix(s: &str) -> &str {
    s.strip_prefix("session:").unwrap_or(s)
}

fn ensure_session_prefix(s: &str) -> String {
    if s.starts_with("session:") {
        s.to_string()
    } else {
        format!("session:{s}")
    }
}

/// Cheap helper for callers that want a fresh empty map (used in tests).
pub fn empty_breakdown() -> HashMap<String, i64> {
    HashMap::new()
}
