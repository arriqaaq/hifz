//! Write-time and query-time edge generation for the knowledge graph.
//!
//! On each new `memory` row, we look for related existing rows via three
//! deterministic channels and emit signal-typed `co_occurs_*` edges:
//!   - embedding KNN cosine    -> `co_occurs_embedding`
//!   - keyword Jaccard         -> `co_occurs_keywords`
//!   - file Jaccard            -> `co_occurs_files`
//!
//! Entity-based links plug in via `relation='mentions', via='entity'`.
//! Causal/provenance edges (`derived_from`, `informed_by`, `generated_by`,
//! `part_of`, `follows`) are created by callers at the appropriate lifecycle
//! points. LLM-set conceptual / argumentative relations come from `evolve.rs`
//! (Phase 2 will fold this into a single insert pipeline).
//!
//! Every edge insert is type-pair validated via `is_allowed_relation`.

use std::collections::HashSet;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::models::{EdgeRelation, RecordKind};

const EMBEDDING_DISTANCE_MAX: f64 = 0.25;
const KNN_K: usize = 10;
const KNN_EF: usize = 100;

/// Maximum number of co-occurrence edges of one channel emitted per insert.
/// Caps graph density growth in popular projects.
const COOCCUR_CAP_PER_CHANNEL: usize = 5;

/// Score keyword/file overlap honestly.
///
/// Returns `None` when the overlap is too weak to claim a relation:
///   - 0 shared items: no relation.
///   - 1 shared item but both sides have > 2 items: probably noise (one
///     popular term like `auth` linking everything).
///
/// Otherwise returns `Some(score)` where score is `min(shared / 2.0, 1.0)`.
/// Two shared items is the floor for a confident edge (score 1.0).
pub fn overlap_score(shared: usize, size_a: usize, size_b: usize) -> Option<f64> {
    if shared == 0 {
        return None;
    }
    if shared == 1 && size_a > 2 && size_b > 2 {
        return None;
    }
    Some((shared as f64 / 2.0).min(1.0))
}

#[derive(Debug, SurrealValue)]
struct CandidateRow {
    id: Option<RecordId>,
    distance: Option<f64>,
    keywords: Option<Vec<String>>,
    files: Option<Vec<String>>,
}

#[derive(Debug, Default)]
pub struct LinkReport {
    pub similarity_links: usize,
    pub entity_links: usize,
}

/// Generate co-occurrence links for a freshly-written memory.
///
/// Emits three signal-typed edges, one per channel: `co_occurs_embedding`,
/// `co_occurs_keywords`, `co_occurs_files`. The relation name records *what*
/// produced the edge; the `reason` field records the score and shared items.
pub async fn generate_links(
    db: &Surreal<Db>,
    self_id: &RecordId,
    project: &str,
    embedding: &[f32],
    keywords: &[String],
    files: &[String],
) -> Result<LinkReport> {
    let mut report = LinkReport::default();

    let sql = format!(
        "SELECT id, vector::distance::knn() AS distance, keywords, files \
         FROM memory \
         WHERE is_latest = true \
           AND id != $self \
           AND (project = $project OR project = 'global') \
           AND embedding <|{KNN_K},{KNN_EF}|> $vec"
    );
    let mut resp = db
        .query(&sql)
        .bind(("self", self_id.clone()))
        .bind(("project", project.to_string()))
        .bind(("vec", embedding.to_vec()))
        .await?;
    let candidates: Vec<CandidateRow> = resp.take(0).unwrap_or_default();

    let self_keywords: HashSet<&str> = keywords.iter().map(String::as_str).collect();
    let self_files: HashSet<&str> = files.iter().map(String::as_str).collect();

    // Score every candidate per channel first so we can keep only the top-N
    // strongest edges per channel — caps graph density in busy projects.
    struct Hit<'a> {
        other_id: RecordId,
        score: f64,
        reason: String,
        relation: &'static str,
        via: &'static str,
        _marker: std::marker::PhantomData<&'a ()>,
    }
    let mut emb_hits: Vec<Hit> = Vec::new();
    let mut kw_hits: Vec<Hit> = Vec::new();
    let mut file_hits: Vec<Hit> = Vec::new();

    for c in &candidates {
        let Some(other_id) = c.id.clone() else {
            continue;
        };

        if let Some(d) = c.distance
            && d < EMBEDDING_DISTANCE_MAX
        {
            let score = (1.0 - d).clamp(0.0, 1.0);
            emb_hits.push(Hit {
                other_id: other_id.clone(),
                score,
                reason: format!("cosine sim {score:.3} (dist {d:.3})"),
                relation: "co_occurs_embedding",
                via: "embedding",
                _marker: std::marker::PhantomData,
            });
        }

        if !self_keywords.is_empty() {
            let other: HashSet<&str> = c
                .keywords
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let shared: Vec<&str> = self_keywords.intersection(&other).copied().collect();
            if let Some(score) = overlap_score(shared.len(), self_keywords.len(), other.len()) {
                kw_hits.push(Hit {
                    other_id: other_id.clone(),
                    score,
                    reason: format!(
                        "keyword overlap {} shared (score {:.2}): {}",
                        shared.len(),
                        score,
                        shared.join(", ")
                    ),
                    relation: "co_occurs_keywords",
                    via: "keyword",
                    _marker: std::marker::PhantomData,
                });
            }
        }

        if !self_files.is_empty() {
            let other: HashSet<&str> = c
                .files
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let shared: Vec<&str> = self_files.intersection(&other).copied().collect();
            if let Some(score) = overlap_score(shared.len(), self_files.len(), other.len()) {
                file_hits.push(Hit {
                    other_id: other_id.clone(),
                    score,
                    reason: format!(
                        "file overlap {} shared (score {:.2}): {}",
                        shared.len(),
                        score,
                        shared.join(", ")
                    ),
                    relation: "co_occurs_files",
                    via: "file",
                    _marker: std::marker::PhantomData,
                });
            }
        }
    }

    for hits in [&mut emb_hits, &mut kw_hits, &mut file_hits] {
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(COOCCUR_CAP_PER_CHANNEL);
        for h in hits.iter() {
            upsert_edge(
                db,
                self_id,
                &h.other_id,
                h.relation,
                h.via,
                h.score,
                Some(&h.reason),
            )
            .await?;
            report.similarity_links += 1;
        }
    }

    Ok(report)
}

/// Extract the `RecordKind` for a `RecordId` by inspecting its Debug form.
/// SurrealDB's RecordId Debug format is `table:key`; we split on the first
/// non-identifier char to get the table name.
fn record_kind_of(rid: &RecordId) -> RecordKind {
    let s = format!("{rid:?}");
    let table: String = s
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    RecordKind::from_table(&table)
}

/// Reject `(from_kind, relation, to_kind)` triples that violate the ontology.
/// Returns true if the edge is permitted; false to skip with a warning.
///
/// Unknown / `Other` relations are accepted (forward-compat with LLM-proposed
/// labels we may not have enumerated yet). Unknown record kinds (`Other`)
/// are also accepted to avoid blocking inserts when the table list grows.
pub fn is_allowed_relation(rel: EdgeRelation, from: RecordKind, to: RecordKind) -> bool {
    use EdgeRelation as R;
    use RecordKind as K;
    // `Other` exists on both enums — qualify both to dodge glob-import ambiguity.
    if matches!(rel, R::Other) || from == K::Other || to == K::Other {
        return true;
    }
    match rel {
        // Co-occurrence: memory<->memory (and observation->memory in Phase 6 use of Mentions).
        R::CoOccursFiles | R::CoOccursKeywords | R::CoOccursEmbedding => {
            from == K::Memory && to == K::Memory
        }
        R::Mentions => from == K::Memory && to == K::Memory,

        // Provenance: PROV-O shapes. Both Memory and Observation count as
        // entities that can be generated by a Run (activity).
        R::GeneratedBy => (from == K::Memory || from == K::Observation) && to == K::Run,
        R::InformedBy => from == K::Memory && to == K::Run,
        R::DerivedFrom => from == K::Memory && (to == K::Memory || to == K::Observation),
        R::AttributedTo => from == K::Memory,
        R::PartOf => {
            (from == K::Run && to == K::Session)
                || (from == K::Memory && to == K::Memory)
                || (from == K::Observation && to == K::Run)
                // Code dimension structural containment.
                || (from == K::CodeChunk && to == K::CodeFile)
                || (from == K::CodeSymbol && to == K::CodeFile)
                // Resolves the existing TODO at chunk.rs:239 — make MemoryChunk
                // → Memory containment a first-class allowed edge instead of
                // relying on the `Other`-passthrough.
                || (from == K::MemoryChunk && to == K::Memory)
        }
        R::Follows => from == K::Run && to == K::Run,

        // Conceptual: memory<->memory only.
        R::Broader | R::Narrower | R::Related | R::SameAs => from == K::Memory && to == K::Memory,

        // Argumentative: memory<->memory only.
        R::Supports | R::Contradicts | R::Elaborates | R::RespondsTo => {
            from == K::Memory && to == K::Memory
        }

        // Lifecycle: memory<->memory.
        R::Supersedes | R::Closes => from == K::Memory && to == K::Memory,

        // Code-domain: observation->memory mostly.
        R::TouchesFile => from == K::Observation && to == K::Memory,
        R::CommitsFor => from == K::Observation && to == K::Memory,
        R::Tests => from == K::Memory && to == K::Memory,

        // Memory points at a precise location/symbol in code.
        R::References => from == K::Memory && (to == K::CodeChunk || to == K::CodeFile),
        R::ReferencesSymbol => from == K::Memory && to == K::CodeSymbol,

        // Phase 0b code-graph: symbol→symbol structural / call / import edges
        // from the code-intelligence core. External targets (`external_symbol`
        // table) ride the `to == K::Other` passthrough at the top of this fn
        // until E4 promotes `ExternalSymbol` to a first-class `RecordKind`
        // (mirrors the MemoryChunk precedent in the `PartOf` arm above).
        R::Calls => from == K::CodeSymbol && to == K::CodeSymbol,
        R::Imports => {
            (from == K::CodeFile || from == K::CodeSymbol)
                && (to == K::CodeFile || to == K::CodeSymbol)
        }
        R::Contains => (from == K::CodeFile || from == K::CodeSymbol) && to == K::CodeSymbol,

        R::Other => true,
    }
}

/// Upsert a single edge with per-(relation, via) dedup and max-score merge.
///
/// Validates the `(from_kind, relation, to_kind)` triple via
/// `is_allowed_relation`. Violations are logged at WARN level and skipped (the
/// call returns `Ok(())` so writers don't fail catastrophically). Phase 10 may
/// flip this to a hard error once the call sites are proven clean.
///
/// `reason` is a one-line justification stored on the edge: deterministic
/// callers pass a channel+score string; LLM callers pass the model's rationale.
pub async fn upsert_edge(
    db: &Surreal<Db>,
    from: &RecordId,
    to: &RecordId,
    relation: &str,
    via: &str,
    score: f64,
    reason: Option<&str>,
) -> Result<()> {
    let rel_typed = EdgeRelation::from_str(relation);
    let from_kind = record_kind_of(from);
    let to_kind = record_kind_of(to);
    if !is_allowed_relation(rel_typed, from_kind, to_kind) {
        tracing::warn!(
            "edge type-pair rejected: {from_kind:?} --{relation}--> {to_kind:?} (from={from:?}, to={to:?})"
        );
        return Ok(());
    }

    #[derive(Debug, SurrealValue)]
    struct Existing {
        id: Option<RecordId>,
        score: Option<f64>,
    }

    let mut resp = db
        .query(
            "SELECT id, score FROM edge \
             WHERE in = $from AND out = $to AND relation = $rel AND via = $via \
             LIMIT 1",
        )
        .bind(("from", from.clone()))
        .bind(("to", to.clone()))
        .bind(("rel", relation.to_string()))
        .bind(("via", via.to_string()))
        .await?;
    let existing: Vec<Existing> = resp.take(0).unwrap_or_default();

    if let Some(row) = existing.into_iter().next() {
        if let Some(id) = row.id {
            let old = row.score.unwrap_or(0.0);
            if score > old {
                // Refresh both score and reason when we beat the prior score —
                // the most-recent strongest signal is what audit consumers want.
                db.query("UPDATE type::record($id) SET score = $score, reason = $reason")
                    .bind(("id", id))
                    .bind(("score", score))
                    .bind(("reason", reason.map(str::to_string)))
                    .await?
                    .check()?;
            }
        }
        return Ok(());
    }

    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "RELATE $from->edge->$to SET \
         relation = $rel, via = $via, score = $score, reason = $reason, created_at = $now",
    )
    .bind(("from", from.clone()))
    .bind(("to", to.clone()))
    .bind(("rel", relation.to_string()))
    .bind(("via", via.to_string()))
    .bind(("score", score))
    .bind(("reason", reason.map(str::to_string)))
    .bind(("now", now))
    .await?
    .check()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Graph expansion at retrieval time
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EdgeHit {
    pub from: RecordId,
    pub to: RecordId,
    pub score: f64,
    pub relation: String,
    pub via: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Direction {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone)]
pub struct GraphExpandConfig {
    pub max_hops: usize,
    pub relations: Option<Vec<String>>,
    pub min_score: f64,
    pub dampening: f64,
    pub max_results: usize,
    pub direction: Direction,
}

impl Default for GraphExpandConfig {
    fn default() -> Self {
        Self {
            max_hops: 2,
            relations: None,
            min_score: 0.0,
            dampening: 0.5,
            max_results: 20,
            direction: Direction::Outgoing,
        }
    }
}

/// Fetch edges from the given seed ids, with optional relation filtering.
/// Supports multi-hop traversal with dampened scoring.
pub async fn expand_graph(
    db: &Surreal<Db>,
    seed_ids: &[RecordId],
    config: &GraphExpandConfig,
) -> Result<Vec<EdgeHit>> {
    if seed_ids.is_empty() || config.max_hops == 0 {
        return Ok(vec![]);
    }

    let mut all_edges: Vec<EdgeHit> = Vec::new();
    let mut current_seeds: Vec<RecordId> = seed_ids.to_vec();
    let mut visited: HashSet<String> = seed_ids.iter().map(|id| format!("{id:?}")).collect();

    for _hop in 0..config.max_hops {
        if current_seeds.is_empty() {
            break;
        }

        let hop_edges = fetch_edges(
            db,
            &current_seeds,
            &config.relations,
            config.min_score,
            &config.direction,
        )
        .await?;
        if hop_edges.is_empty() {
            break;
        }

        let mut next_seeds = Vec::new();
        for e in hop_edges {
            let (neighbor_key, neighbor_rid) = match config.direction {
                Direction::Incoming => (format!("{:?}", e.from), e.from.clone()),
                _ => (format!("{:?}", e.to), e.to.clone()),
            };
            if visited.insert(neighbor_key) {
                next_seeds.push(neighbor_rid);
                all_edges.push(e);
            }
        }

        if all_edges.len() >= config.max_results {
            all_edges.truncate(config.max_results);
            break;
        }

        current_seeds = next_seeds;
    }

    Ok(all_edges)
}

async fn fetch_edges(
    db: &Surreal<Db>,
    ids: &[RecordId],
    relations: &Option<Vec<String>>,
    min_score: f64,
    direction: &Direction,
) -> Result<Vec<EdgeHit>> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        #[surreal(rename = "in")]
        in_: Option<RecordId>,
        out: Option<RecordId>,
        score: Option<f64>,
        relation: Option<String>,
        via: Option<String>,
        reason: Option<String>,
    }

    let (direction_clause, bind_field) = match direction {
        Direction::Outgoing => ("in IN $ids", "ids"),
        Direction::Incoming => ("out IN $ids", "ids"),
        Direction::Both => ("(in IN $ids OR out IN $ids)", "ids"),
    };

    let rel_clause = if let Some(rels) = relations {
        if rels.is_empty() {
            String::new()
        } else {
            " AND relation IN $rels".to_string()
        }
    } else {
        String::new()
    };

    let sql = format!(
        "SELECT in, out, score, relation, via, reason FROM edge WHERE {direction_clause}{rel_clause} AND score >= $min"
    );

    let mut query = db
        .query(&sql)
        .bind((bind_field, ids.to_vec()))
        .bind(("min", min_score));
    if let Some(rels) = relations
        && !rels.is_empty()
    {
        query = query.bind(("rels", rels.clone()));
    }

    let mut resp = query.await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(EdgeHit {
                from: r.in_?,
                to: r.out?,
                score: r.score.unwrap_or(0.0),
                relation: r.relation.unwrap_or_default(),
                via: r.via.unwrap_or_default(),
                reason: r.reason,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Lifecycle edge helpers
// ---------------------------------------------------------------------------

/// Create structural edges when a run closes:
/// - run --part_of--> session
/// - run --follows--> previous completed run in same session
pub async fn create_run_structure_edges(
    db: &Surreal<Db>,
    run_id: &RecordId,
    session_id: &RecordId,
) -> Result<()> {
    upsert_edge(
        db,
        run_id,
        session_id,
        "part_of",
        "system",
        1.0,
        Some("run belongs to session (lifecycle)"),
    )
    .await?;

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
        #[allow(dead_code)]
        ended_at: Option<String>,
    }
    let mut resp = db
        .query(
            "SELECT id, ended_at FROM run \
             WHERE session_id = $sid AND id != $rid AND ended_at IS NOT NONE \
             ORDER BY ended_at DESC LIMIT 1",
        )
        .bind(("sid", session_id.clone()))
        .bind(("rid", run_id.clone()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    if let Some(prev_id) = rows.into_iter().next().and_then(|r| r.id) {
        upsert_edge(
            db,
            run_id,
            &prev_id,
            "follows",
            "system",
            1.0,
            Some("temporal sequence within session"),
        )
        .await?;
    }

    Ok(())
}

// --- Memory links listing (lifted from web/api.rs::memory_links) ---

/// List outbound graph edges from a memory. Returns the legacy wire shape
/// `{"links": [...], "count": N}` with each link's title/category/relation/score.
pub async fn list_for(
    db: &surrealdb::Surreal<crate::db::Db>,
    memory_id: &str,
) -> anyhow::Result<serde_json::Value> {
    let id = if memory_id.starts_with("memory:") {
        memory_id.to_string()
    } else {
        format!("memory:{memory_id}")
    };

    let mut resp = db
        .query(
            "SELECT out.id AS id, out.title AS title, out.category AS category, \
             relation, score, via, reason FROM edge WHERE in = type::record($id)",
        )
        .bind(("id", id))
        .await?;

    let links: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let count = links.len();
    Ok(serde_json::json!({"links": links, "count": count}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_zero_shared_is_none() {
        assert_eq!(overlap_score(0, 3, 3), None);
    }

    #[test]
    fn overlap_one_shared_two_large_sets_is_noise() {
        // single popular term linking unrelated memories — reject.
        assert_eq!(overlap_score(1, 5, 4), None);
    }

    #[test]
    fn overlap_one_shared_small_set_is_kept() {
        // when one side is tiny, a single shared item is meaningful.
        assert_eq!(overlap_score(1, 1, 4), Some(0.5));
        assert_eq!(overlap_score(1, 4, 2), Some(0.5));
    }

    #[test]
    fn overlap_two_shared_floor_is_full_score() {
        assert_eq!(overlap_score(2, 5, 5), Some(1.0));
    }

    #[test]
    fn overlap_capped_at_one() {
        assert_eq!(overlap_score(10, 10, 10), Some(1.0));
    }

    #[test]
    fn allowed_relation_blocks_observation_contradicts_run() {
        let allowed = is_allowed_relation(
            EdgeRelation::Contradicts,
            RecordKind::Observation,
            RecordKind::Run,
        );
        assert!(!allowed);
    }

    #[test]
    fn allowed_relation_permits_observation_generated_by_run() {
        // Phase 1: PROV-O permits Memory|Observation -> Run via generated_by.
        let allowed = is_allowed_relation(
            EdgeRelation::GeneratedBy,
            RecordKind::Observation,
            RecordKind::Run,
        );
        assert!(allowed);
    }

    #[test]
    fn allowed_relation_passthrough_for_other() {
        let allowed = is_allowed_relation(EdgeRelation::Other, RecordKind::Memory, RecordKind::Run);
        assert!(allowed);
    }
}
