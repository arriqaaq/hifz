//! Pure aggregation helpers over `agent_usage` rows.
//!
//! Reading is done once into `UsageCallRow` and then every roll-up
//! (daily, model, project, top-prompts) runs in-memory. This avoids
//! N round-trips and keeps the math obvious — at the row counts we
//! expect (<100K per project) it costs nothing.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::db::Db;
use crate::usage::ProjectFilters;

/// Default per-million-token prices keyed by model-name prefix. Used when
/// the user has not supplied an override at `~/.hifz/prices.json`. Tuple
/// is `(prefix, input, output, cache_read, cache_creation)` in USD per
/// million tokens. List is matched longest-prefix first.
///
/// Source: Anthropic's published pricing as of 2026-01. Update via the
/// override file rather than editing this list — the override survives
/// re-deploys and lets the user pin historical price tables.
const DEFAULT_PRICES: &[(&str, f64, f64, f64, f64)] = &[
    // Claude 4.x
    ("claude-opus-4", 15.0, 75.0, 1.5, 18.75),
    ("claude-sonnet-4", 3.0, 15.0, 0.3, 3.75),
    ("claude-haiku-4", 1.0, 5.0, 0.1, 1.25),
    // Claude 3.5 (legacy but still in use)
    ("claude-3-5-sonnet", 3.0, 15.0, 0.3, 3.75),
    ("claude-3-5-haiku", 0.8, 4.0, 0.08, 1.0),
    ("claude-3-opus", 15.0, 75.0, 1.5, 18.75),
];

#[derive(Debug, Clone, Copy, Default)]
struct PriceRow {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_creation: f64,
}

static PRICES: OnceLock<Vec<(String, PriceRow)>> = OnceLock::new();

fn prices_table() -> &'static Vec<(String, PriceRow)> {
    PRICES.get_or_init(|| {
        let mut out: Vec<(String, PriceRow)> = DEFAULT_PRICES
            .iter()
            .map(|(prefix, i, o, cr, cc)| {
                (
                    (*prefix).to_string(),
                    PriceRow {
                        input: *i,
                        output: *o,
                        cache_read: *cr,
                        cache_creation: *cc,
                    },
                )
            })
            .collect();
        // Optional override: ~/.hifz/prices.json — same shape as the const
        // above but as a JSON object: `{ "claude-sonnet-4": {"input":3, ...} }`.
        // Anything in the override either replaces the matching prefix or
        // adds a new one. We log+ignore parse errors so a malformed file
        // never blocks startup.
        if let Some(home) = std::env::var_os("HOME") {
            let path = std::path::Path::new(&home).join(".hifz/prices.json");
            if let Ok(raw) = std::fs::read_to_string(&path) {
                match serde_json::from_str::<HashMap<String, PriceOverride>>(&raw) {
                    Ok(overrides) => {
                        for (prefix, ov) in overrides {
                            let row = PriceRow {
                                input: ov.input,
                                output: ov.output,
                                cache_read: ov.cache_read,
                                cache_creation: ov.cache_creation,
                            };
                            if let Some(slot) = out.iter_mut().find(|(p, _)| p == &prefix) {
                                slot.1 = row;
                            } else {
                                out.push((prefix, row));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "skipping ~/.hifz/prices.json");
                    }
                }
            }
        }
        // Sort by descending prefix length so longest-match wins.
        out.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        out
    })
}

#[derive(Debug, Clone, Deserialize)]
struct PriceOverride {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_creation: f64,
}

fn price_for(model: &str) -> Option<PriceRow> {
    if model.is_empty() || model == "unknown" {
        return None;
    }
    prices_table()
        .iter()
        .find(|(prefix, _)| model.starts_with(prefix))
        .map(|(_, p)| *p)
}

/// One row in the result of an `agent_usage` query — a single LLM call.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct UsageCallRow {
    pub id: String,
    pub session_id: String,
    pub project: String,
    pub agent: String,
    pub model: String,
    pub external_id: String,
    pub timestamp: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub prompt: Option<String>,
    pub tools: Vec<String>,
    pub breakdown: Option<serde_json::Value>,
    pub aux_calls: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenTotals {
    pub input: i64,
    pub output: i64,
    /// Sum of `input + output + cache_read + cache_creation` — the raw
    /// activity volume. NOT a cost-equivalent quantity (cache_read tokens
    /// cost ~10× less than `input`, so this number badly overstates spend
    /// on cache-heavy workloads). Use `cost_usd` for spend.
    pub total: i64,
    /// Aggregated `breakdown` keys → summed values. Empty when no
    /// breakdown was provided by any adapter.
    pub breakdown: HashMap<String, i64>,
    /// Sum of all `breakdown` values whose key starts with `cache_read` —
    /// the universal "served from cache" notion. Adapters that name their
    /// cached-input field differently can also expose it under
    /// `breakdown.cache_read`.
    pub cache_read: i64,
    /// Same for `cache_creation`-style keys.
    pub cache_creation: i64,
    pub cache_hit_rate: f64,
    /// Server-side billable-equivalent cost, summed per call against the
    /// embedded `PRICES` table (overridable at `~/.hifz/prices.json`).
    /// Calls whose model has no entry contribute `0` and bump
    /// `cost_unknown_calls`.
    pub cost_usd: f64,
    pub cost_unknown_calls: i64,
    /// Sum of `aux_calls` across rows — auxiliary Anthropic calls
    /// (ai-title, summary) that were billed but excluded from the JSONL
    /// transcript. Surfaced in the UI as a footer badge; never folded
    /// into token counts (we don't know their token counts).
    pub aux_calls: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateRange {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyBucket {
    pub date: String,
    pub input: i64,
    pub output: i64,
    pub total: i64,
    /// `breakdown[key]` summed for this day. Empty when no breakdown
    /// data was recorded.
    pub breakdown: HashMap<String, i64>,
    pub calls: i64,
    pub sessions: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelBucket {
    pub model: String,
    pub total: i64,
    pub input: i64,
    pub output: i64,
    pub calls: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRow {
    pub prompt: String,
    pub total: i64,
    pub input: i64,
    pub output: i64,
    pub breakdown: HashMap<String, i64>,
    pub session_id: String,
    pub model: String,
    pub date: String,
    pub call_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub session_id: String,
    pub first_prompt: Option<String>,
    pub total: i64,
    pub input: i64,
    pub output: i64,
    pub calls: i64,
    pub model: String,
    pub date: String,
}

// queries

pub async fn calls_for_session(db: &Surreal<Db>, session_id: &str) -> Result<Vec<UsageCallRow>> {
    let sid = session_id
        .strip_prefix("session:")
        .unwrap_or(session_id)
        .to_string();
    let mut resp = db
        .query(
            "SELECT
                record::id(id) AS id,
                record::id(session_id) AS session_id,
                project, agent, model, external_id, timestamp,
                input_tokens, output_tokens, total_tokens,
                prompt, tools, breakdown, aux_calls
             FROM agent_usage
             WHERE record::id(session_id) = $sid
             ORDER BY timestamp ASC;",
        )
        .bind(("sid", sid))
        .await?;
    let rows: Vec<UsageCallRow> = resp.take(0).unwrap_or_default();
    Ok(rows)
}

pub async fn calls_for_project(
    db: &Surreal<Db>,
    project: &str,
    filters: &ProjectFilters,
) -> Result<Vec<UsageCallRow>> {
    let mut sql = String::from(
        "SELECT
            record::id(id) AS id,
            record::id(session_id) AS session_id,
            project, agent, model, external_id, timestamp,
            input_tokens, output_tokens, total_tokens,
            prompt, tools, breakdown, aux_calls
         FROM agent_usage
         WHERE project = $project",
    );
    if filters.from.is_some() {
        sql.push_str(" AND timestamp >= $from");
    }
    if filters.to.is_some() {
        sql.push_str(" AND timestamp <= $to");
    }
    if filters.model.is_some() {
        sql.push_str(" AND model = $model");
    }
    sql.push_str(" ORDER BY timestamp ASC;");

    let mut q = db.query(sql).bind(("project", project.to_string()));
    if let Some(f) = &filters.from {
        q = q.bind(("from", f.clone()));
    }
    if let Some(t) = &filters.to {
        q = q.bind(("to", t.clone()));
    }
    if let Some(m) = &filters.model {
        q = q.bind(("model", m.clone()));
    }
    let mut resp = q.await?;
    let rows: Vec<UsageCallRow> = resp.take(0).unwrap_or_default();
    Ok(rows)
}

/// Every usage row across all projects (no project filter). Powers the
/// `/sessions` list column, which spans projects. Same projection as
/// `calls_for_project` so the in-memory rollups can be reused unchanged.
pub async fn calls_all(db: &Surreal<Db>) -> Result<Vec<UsageCallRow>> {
    let mut resp = db
        .query(
            "SELECT
                record::id(id) AS id,
                record::id(session_id) AS session_id,
                project, agent, model, external_id, timestamp,
                input_tokens, output_tokens, total_tokens,
                prompt, tools, breakdown, aux_calls
             FROM agent_usage
             ORDER BY timestamp ASC;",
        )
        .await?;
    let rows: Vec<UsageCallRow> = resp.take(0).unwrap_or_default();
    Ok(rows)
}

// in-memory rollups

pub fn sum_calls(calls: &[UsageCallRow]) -> TokenTotals {
    let mut t = TokenTotals::default();
    for c in calls {
        t.input += c.input_tokens;
        t.output += c.output_tokens;
        t.total += c.total_tokens;
        let mut call_cache_read: i64 = 0;
        let mut call_cache_creation: i64 = 0;
        if let Some(serde_json::Value::Object(map)) = &c.breakdown {
            for (k, v) in map.iter() {
                if let Some(n) = v.as_i64() {
                    *t.breakdown.entry(k.clone()).or_insert(0) += n;
                    if k.starts_with("cache_read") {
                        t.cache_read += n;
                        call_cache_read += n;
                    } else if k.starts_with("cache_creation") {
                        t.cache_creation += n;
                        call_cache_creation += n;
                    }
                }
            }
        }
        t.aux_calls += c.aux_calls.unwrap_or(0);
        match price_for(&c.model) {
            Some(p) => {
                let cost = (c.input_tokens as f64 * p.input
                    + c.output_tokens as f64 * p.output
                    + call_cache_read as f64 * p.cache_read
                    + call_cache_creation as f64 * p.cache_creation)
                    / 1_000_000.0;
                t.cost_usd += cost;
            }
            None => t.cost_unknown_calls += 1,
        }
    }
    let denom = t.input + t.cache_read + t.cache_creation;
    t.cache_hit_rate = if denom > 0 {
        t.cache_read as f64 / denom as f64
    } else {
        0.0
    };
    t
}

pub fn date_range(calls: &[UsageCallRow]) -> Option<DateRange> {
    let mut dates: Vec<&str> = calls
        .iter()
        .map(|c| c.timestamp.get(0..10).unwrap_or(""))
        .filter(|d| !d.is_empty())
        .collect();
    if dates.is_empty() {
        return None;
    }
    dates.sort();
    Some(DateRange {
        from: dates.first().unwrap().to_string(),
        to: dates.last().unwrap().to_string(),
    })
}

pub fn daily_buckets(calls: &[UsageCallRow]) -> Vec<DailyBucket> {
    let mut by_date: HashMap<String, DailyBucket> = HashMap::new();
    let mut sessions_per_date: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    for c in calls {
        let date = match c.timestamp.get(0..10) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => continue,
        };
        let bucket = by_date.entry(date.clone()).or_insert(DailyBucket {
            date: date.clone(),
            input: 0,
            output: 0,
            total: 0,
            breakdown: HashMap::new(),
            calls: 0,
            sessions: 0,
        });
        bucket.input += c.input_tokens;
        bucket.output += c.output_tokens;
        bucket.total += c.total_tokens;
        bucket.calls += 1;
        if let Some(serde_json::Value::Object(map)) = &c.breakdown {
            for (k, v) in map.iter() {
                if let Some(n) = v.as_i64() {
                    *bucket.breakdown.entry(k.clone()).or_insert(0) += n;
                }
            }
        }
        sessions_per_date
            .entry(date)
            .or_default()
            .insert(c.session_id.clone());
    }
    let mut out: Vec<DailyBucket> = by_date.into_values().collect();
    for d in &mut out {
        d.sessions = sessions_per_date
            .get(&d.date)
            .map(|s| s.len() as i64)
            .unwrap_or(0);
    }
    out.sort_by(|a, b| a.date.cmp(&b.date));
    out
}

pub fn model_buckets(calls: &[UsageCallRow]) -> Vec<ModelBucket> {
    let mut by_model: HashMap<String, ModelBucket> = HashMap::new();
    for c in calls {
        let m = by_model.entry(c.model.clone()).or_insert(ModelBucket {
            model: c.model.clone(),
            total: 0,
            input: 0,
            output: 0,
            calls: 0,
        });
        m.input += c.input_tokens;
        m.output += c.output_tokens;
        m.total += c.total_tokens;
        m.calls += 1;
    }
    let mut out: Vec<ModelBucket> = by_model.into_values().collect();
    out.sort_by(|a, b| b.total.cmp(&a.total));
    out
}

/// Group consecutive calls sharing the same `prompt` into "turns,"
/// then return the heaviest `limit` of them. Mirrors claude-spend's
/// `allPrompts` flush logic.
pub fn top_prompts(calls: &[UsageCallRow], limit: usize) -> Vec<PromptRow> {
    let mut out: Vec<PromptRow> = Vec::new();
    let mut cur: Option<PromptRow> = None;
    for c in calls {
        match c.prompt.as_ref() {
            Some(p) if !p.is_empty() && cur.as_ref().map(|r| &r.prompt) != Some(p) => {
                if let Some(row) = cur.take() {
                    out.push(row);
                }
                cur = Some(PromptRow {
                    prompt: truncate(p, 300),
                    total: 0,
                    input: 0,
                    output: 0,
                    breakdown: HashMap::new(),
                    session_id: c.session_id.clone(),
                    model: c.model.clone(),
                    date: c.timestamp.get(0..10).unwrap_or("").to_string(),
                    call_count: 0,
                });
            }
            _ => {}
        }
        let row = match cur.as_mut() {
            Some(r) => r,
            None => continue, // calls before any prompt — skip
        };
        row.total += c.total_tokens;
        row.input += c.input_tokens;
        row.output += c.output_tokens;
        row.call_count += 1;
        if let Some(serde_json::Value::Object(map)) = &c.breakdown {
            for (k, v) in map.iter() {
                if let Some(n) = v.as_i64() {
                    *row.breakdown.entry(k.clone()).or_insert(0) += n;
                }
            }
        }
    }
    if let Some(row) = cur {
        out.push(row);
    }
    out.sort_by(|a, b| b.total.cmp(&a.total));
    out.truncate(limit);
    out
}

pub fn top_sessions(calls: &[UsageCallRow], limit: usize) -> Vec<SessionRow> {
    let mut by_session: HashMap<String, SessionRow> = HashMap::new();
    let mut first_prompt: HashMap<String, String> = HashMap::new();
    let mut first_ts: HashMap<String, String> = HashMap::new();
    let mut model_counts: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for c in calls {
        let s = by_session
            .entry(c.session_id.clone())
            .or_insert(SessionRow {
                session_id: c.session_id.clone(),
                first_prompt: None,
                total: 0,
                input: 0,
                output: 0,
                calls: 0,
                model: String::new(),
                date: c.timestamp.get(0..10).unwrap_or("").to_string(),
            });
        s.total += c.total_tokens;
        s.input += c.input_tokens;
        s.output += c.output_tokens;
        s.calls += 1;
        if !first_ts.contains_key(&c.session_id) {
            first_ts.insert(c.session_id.clone(), c.timestamp.clone());
            s.date = c.timestamp.get(0..10).unwrap_or("").to_string();
        }
        if let Some(p) = &c.prompt {
            first_prompt
                .entry(c.session_id.clone())
                .or_insert_with(|| truncate(p, 200));
        }
        let mc = model_counts.entry(c.session_id.clone()).or_default();
        *mc.entry(c.model.clone()).or_insert(0) += 1;
    }
    for (sid, row) in by_session.iter_mut() {
        row.first_prompt = first_prompt.remove(sid);
        row.model = model_counts
            .get(sid)
            .and_then(|m| m.iter().max_by_key(|(_, v)| *v).map(|(k, _)| k.clone()))
            .unwrap_or_default();
    }
    let mut out: Vec<SessionRow> = by_session.into_values().collect();
    out.sort_by(|a, b| b.total.cmp(&a.total));
    out.truncate(limit);
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}
