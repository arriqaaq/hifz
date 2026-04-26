//! A-MEM Memory Evolution — opt-in LLM-driven neighbour refinement.
//!
//! Gated by `HIFZ_LLM_EVOLVE=true` at startup. On a new-memory write (or manual
//! `hifz_evolve` invocation), the LLM inspects the new note and its KNN/graph
//! neighbours, then emits JSON updates for up to N neighbours — adjusting
//! keywords/tags/context, optionally adding a `via='semantic'` edge, and
//! optionally flagging one as superseded by the other.
//!
//! The deterministic graph (`link.rs`) is already populated by the time
//! evolution runs, so this step is additive; retrieval works fully without it.

use std::collections::HashSet;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::link;
use crate::ollama::OllamaClient;

const DEFAULT_MAX_NEIGHBOURS: usize = 5;

#[derive(Debug, Deserialize)]
struct LlmOutput {
    new_note: Option<NewNotePatch>,
    #[serde(default)]
    neighbour_updates: Vec<NeighbourUpdate>,
}

#[derive(Debug, Deserialize)]
struct NewNotePatch {
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    context: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NeighbourUpdate {
    id: String,
    #[serde(default)]
    keywords_add: Vec<String>,
    #[serde(default)]
    keywords_remove: Vec<String>,
    #[serde(default)]
    tags_add: Vec<String>,
    #[serde(default)]
    tags_remove: Vec<String>,
    context_rewrite: Option<String>,
    link_to_new: Option<LinkToNew>,
    #[serde(default)]
    supersedes_new: bool,
    #[serde(default)]
    superseded_by_new: bool,
}

#[derive(Debug, Deserialize)]
struct LinkToNew {
    create: bool,
    #[serde(default = "default_relation")]
    relation: String,
    #[serde(default = "default_score")]
    score: f64,
}

fn default_relation() -> String {
    "similar_to".to_string()
}
fn default_score() -> f64 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct MemoryRow {
    id: Option<RecordId>,
    title: Option<String>,
    content: Option<String>,
    keywords: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    context: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct EvolveReport {
    pub considered_neighbours: usize,
    pub neighbour_updates_applied: usize,
    pub links_added: usize,
    pub supersedes_applied: usize,
    pub new_note_updated: bool,
}

/// Run evolution against a single memory id.
pub async fn evolve_one(
    db: &Surreal<Db>,
    ollama: &OllamaClient,
    memory_id: &RecordId,
) -> Result<EvolveReport> {
    let mut resp = db
        .query("SELECT id, title, content, keywords, tags, context FROM type::record($id)")
        .bind(("id", memory_id.clone()))
        .await?;
    let new_rows: Vec<MemoryRow> = resp.take(0).unwrap_or_default();
    let Some(new_row) = new_rows.into_iter().next() else {
        return Ok(EvolveReport::default());
    };

    let edges = link::expand_graph(
        db,
        &[memory_id.clone()],
        &link::GraphExpandConfig {
            max_hops: 1,
            direction: link::Direction::Outgoing,
            ..Default::default()
        },
    )
    .await?;
    let mut neighbour_ids: Vec<RecordId> = edges.iter().map(|e| e.to.clone()).collect();
    neighbour_ids.sort_by_key(|r| format!("{r:?}"));
    neighbour_ids.dedup_by_key(|r| format!("{r:?}"));
    neighbour_ids.truncate(DEFAULT_MAX_NEIGHBOURS);

    if neighbour_ids.is_empty() {
        return Ok(EvolveReport {
            considered_neighbours: 0,
            ..Default::default()
        });
    }

    let mut fetch = db
        .query(
            "SELECT id, title, content, keywords, tags, context \
             FROM memory WHERE id IN $ids",
        )
        .bind(("ids", neighbour_ids.clone()))
        .await?;
    let neighbours: Vec<MemoryRow> = fetch.take(0).unwrap_or_default();

    let prompt = build_prompt(&new_row, &neighbours);
    let raw = ollama
        .complete(SYSTEM_PROMPT, &prompt)
        .await
        .context("ollama complete")?;
    let parsed: LlmOutput = parse_json_payload(&raw)?;

    apply_updates(db, memory_id, &neighbour_ids, &parsed).await
}

async fn apply_updates(
    db: &Surreal<Db>,
    new_id: &RecordId,
    candidate_ids: &[RecordId],
    out: &LlmOutput,
) -> Result<EvolveReport> {
    let mut report = EvolveReport {
        considered_neighbours: candidate_ids.len(),
        ..Default::default()
    };

    // Update the new note's own metadata, if any.
    if let Some(nn) = &out.new_note {
        let any = !nn.keywords.is_empty() || !nn.tags.is_empty() || nn.context.is_some();
        if any {
            let _ = db
                .query(
                    "UPDATE type::record($id) SET \
                     keywords = array::distinct(array::concat(keywords, $kw)), \
                     tags = array::distinct(array::concat(tags, $tg)), \
                     context = $ctx",
                )
                .bind(("id", new_id.clone()))
                .bind(("kw", nn.keywords.clone()))
                .bind(("tg", nn.tags.clone()))
                .bind(("ctx", nn.context.clone()))
                .await?
                .check()?;
            report.new_note_updated = true;
        }
    }

    // Precompute a lookup of allowed id strings (e.g. "memory:abc"), so a runaway
    // LLM can't mutate arbitrary memories.
    let allowed_strs: HashSet<String> = candidate_ids.iter().map(|r| format!("{r:?}")).collect();

    // Pull canonical id strings for the candidates so we can match LLM-quoted ids.
    let canonical_ids = fetch_id_strings(db, candidate_ids)
        .await
        .unwrap_or_default();

    for upd in &out.neighbour_updates {
        let id_str = upd.id.trim().to_string();
        if id_str.is_empty() {
            continue;
        }
        if !allowed_strs.contains(&format!("\"{}\"", id_str)) && !canonical_ids.contains(&id_str) {
            // Accept when the id string matches what we fetched from the DB;
            // otherwise skip.
            continue;
        }

        let touched = !upd.keywords_add.is_empty()
            || !upd.keywords_remove.is_empty()
            || !upd.tags_add.is_empty()
            || !upd.tags_remove.is_empty()
            || upd.context_rewrite.is_some();

        if touched {
            let _ = db
                .query(
                    "UPDATE type::record($id) SET \
                     keywords = array::distinct(array::difference(array::concat(keywords, $ka), $kr)), \
                     tags = array::distinct(array::difference(array::concat(tags, $ta), $tr)), \
                     context = IF $ctx IS NONE THEN context ELSE $ctx END",
                )
                .bind(("id", id_str.clone()))
                .bind(("ka", upd.keywords_add.clone()))
                .bind(("kr", upd.keywords_remove.clone()))
                .bind(("ta", upd.tags_add.clone()))
                .bind(("tr", upd.tags_remove.clone()))
                .bind(("ctx", upd.context_rewrite.clone()))
                .await?
                .check()?;
            report.neighbour_updates_applied += 1;
        }

        // For link + supersedes we need a RecordId — round-trip through SurrealQL.
        let Some(rid) = resolve_id(db, &id_str).await else {
            continue;
        };

        if let Some(ltn) = &upd.link_to_new {
            if ltn.create {
                let score = ltn.score.clamp(0.0, 1.0);
                let relation = if ltn.relation.is_empty() {
                    "similar_to"
                } else {
                    &ltn.relation
                };
                link::upsert_edge(db, &rid, new_id, relation, "llm", score).await?;
                report.links_added += 1;
            }
        }

        if upd.supersedes_new {
            mark_superseded(db, new_id, &rid).await?;
            report.supersedes_applied += 1;
        } else if upd.superseded_by_new {
            mark_superseded(db, &rid, new_id).await?;
            report.supersedes_applied += 1;
        }
    }

    Ok(report)
}

/// Pull stringified ids for a set of RecordIds so we can compare against
/// LLM-quoted id strings without building RecordIds in Rust.
async fn fetch_id_strings(db: &Surreal<Db>, ids: &[RecordId]) -> Result<HashSet<String>> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        id_str: Option<String>,
    }
    let mut resp = db
        .query("SELECT string::concat(meta::tb(id), ':', meta::id(id)) AS id_str FROM memory WHERE id IN $ids")
        .bind(("ids", ids.to_vec()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    Ok(rows.into_iter().filter_map(|r| r.id_str).collect())
}

/// Resolve a "table:key" string into a `RecordId` via round-trip.
async fn resolve_id(db: &Surreal<Db>, id_str: &str) -> Option<RecordId> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query("SELECT id FROM type::record($id)")
        .bind(("id", id_str.to_string()))
        .await
        .ok()?;
    let rows: Vec<Row> = resp.take(0).ok()?;
    rows.into_iter().next().and_then(|r| r.id)
}

/// Mark `older` as superseded by `newer` — mirrors the contradiction logic
/// already in `src/forget.rs`.
async fn mark_superseded(db: &Surreal<Db>, older: &RecordId, newer: &RecordId) -> Result<()> {
    db.query(
        "UPDATE type::record($old) SET is_latest = false, \
         supersedes = array::distinct(array::concat(supersedes ?? [], [$new]))",
    )
    .bind(("old", older.clone()))
    .bind(("new", newer.clone()))
    .await?
    .check()?;
    Ok(())
}

fn parse_json_payload(raw: &str) -> Result<LlmOutput> {
    // Extract the first {...} block — models often wrap JSON in prose.
    let (start, end) = match (raw.find('{'), raw.rfind('}')) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return Err(anyhow::anyhow!("no JSON object in LLM output")),
    };
    let slice = &raw[start..=end];
    serde_json::from_str::<LlmOutput>(slice).context("parse evolution JSON")
}

fn build_prompt(new_row: &MemoryRow, neighbours: &[MemoryRow]) -> String {
    let mut s = String::new();
    s.push_str("NEW MEMORY:\n");
    s.push_str(&format!(
        "- id: {:?}\n  title: {}\n  content: {}\n\n",
        new_row.id,
        new_row.title.as_deref().unwrap_or(""),
        new_row.content.as_deref().unwrap_or("")
    ));
    s.push_str("NEIGHBOURS:\n");
    for n in neighbours {
        s.push_str(&format!(
            "- id: {:?}\n  title: {}\n  content: {}\n  keywords: {:?}\n  tags: {:?}\n  context: {:?}\n\n",
            n.id,
            n.title.as_deref().unwrap_or(""),
            n.content.as_deref().unwrap_or(""),
            n.keywords.clone().unwrap_or_default(),
            n.tags.clone().unwrap_or_default(),
            n.context,
        ));
    }
    s.push_str(
        "Propose metadata updates for neighbours that relate to the new memory. \
         Respond with strict JSON matching the schema described in the system prompt. \
         Do not invent ids that are not in NEIGHBOURS.",
    );
    s
}

const SYSTEM_PROMPT: &str = r#"You are a memory curator.
Given a new note and its graph neighbours, propose updates that keep the
knowledge graph accurate: add keywords/tags, rewrite context lines, create
a semantic link to the new note, or mark a stale neighbour as superseded
by the new note (or vice versa).

Output STRICT JSON (no prose, no code fences). Schema:
{
  "new_note": { "keywords": [str], "tags": [str], "context": str | null } | null,
  "neighbour_updates": [
    {
      "id": "<neighbour-id-from-input>",
      "keywords_add": [str], "keywords_remove": [str],
      "tags_add": [str],     "tags_remove": [str],
      "context_rewrite": str | null,
      "link_to_new": { "create": true, "relation": "similar_to|elaborates|contradicts|supports|depends_on|alternative_to", "score": 0.0..1.0 } | null,
      "supersedes_new": false,
      "superseded_by_new": false
    }
  ]
}

For link_to_new, choose the relation: similar_to (default), elaborates, contradicts,
supports, depends_on, alternative_to.

Only reference ids that appeared in NEIGHBOURS. Keep keyword/tag additions short
(3-6 items) and lower-case. A context_rewrite should be a one-line justification
of why this memory matters relative to the new one."#;
