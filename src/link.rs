//! Write-time and query-time edge generation for the knowledge graph.
//!
//! On each new `memory` row, we look for related existing rows via three
//! channels — embedding KNN, keyword Jaccard, file Jaccard — and `RELATE`
//! them through the generic `edge` table with `relation='similar_to'`.
//!
//! Entity-based links plug in via `relation='mentions', via='entity'`.
//! Causal/provenance edges (`derived_from`, `informed`, `generated_by`, etc.)
//! are created by callers at the appropriate lifecycle points.

use std::collections::HashSet;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

const EMBEDDING_DISTANCE_MAX: f64 = 0.25;
const JACCARD_MIN: f64 = 0.30;
const KNN_K: usize = 10;
const KNN_EF: usize = 100;

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

/// Generate similarity links for a freshly-written memory.
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

    for c in &candidates {
        let Some(other_id) = c.id.clone() else {
            continue;
        };

        if let Some(d) = c.distance {
            if d < EMBEDDING_DISTANCE_MAX {
                let score = (1.0 - d).clamp(0.0, 1.0);
                upsert_edge(db, self_id, &other_id, "similar_to", "embedding", score).await?;
                report.similarity_links += 1;
            }
        }

        if !self_keywords.is_empty() {
            let other: HashSet<&str> = c
                .keywords
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let j = jaccard(&self_keywords, &other);
            if j >= JACCARD_MIN {
                upsert_edge(db, self_id, &other_id, "similar_to", "keyword", j).await?;
                report.similarity_links += 1;
            }
        }

        if !self_files.is_empty() {
            let other: HashSet<&str> = c
                .files
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let j = jaccard(&self_files, &other);
            if j >= JACCARD_MIN {
                upsert_edge(db, self_id, &other_id, "similar_to", "file", j).await?;
                report.similarity_links += 1;
            }
        }
    }

    Ok(report)
}

/// Upsert a single edge with per-(relation, via) dedup and max-score merge.
pub async fn upsert_edge(
    db: &Surreal<Db>,
    from: &RecordId,
    to: &RecordId,
    relation: &str,
    via: &str,
    score: f64,
) -> Result<()> {
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
                db.query("UPDATE type::record($id) SET score = $score")
                    .bind(("id", id))
                    .bind(("score", score))
                    .await?
                    .check()?;
            }
        }
        return Ok(());
    }

    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "RELATE $from->edge->$to SET \
         relation = $rel, via = $via, score = $score, created_at = $now",
    )
    .bind(("from", from.clone()))
    .bind(("to", to.clone()))
    .bind(("rel", relation.to_string()))
    .bind(("via", via.to_string()))
    .bind(("score", score))
    .bind(("now", now))
    .await?
    .check()?;
    Ok(())
}

fn jaccard(a: &HashSet<&str>, b: &HashSet<&str>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
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
            format!(" AND relation IN $rels")
        }
    } else {
        String::new()
    };

    let sql = format!(
        "SELECT in, out, score, relation, via FROM edge WHERE {direction_clause}{rel_clause} AND score >= $min"
    );

    let mut query = db
        .query(&sql)
        .bind((bind_field, ids.to_vec()))
        .bind(("min", min_score));
    if let Some(rels) = relations {
        if !rels.is_empty() {
            query = query.bind(("rels", rels.clone()));
        }
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
    upsert_edge(db, run_id, session_id, "part_of", "system", 1.0).await?;

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query(
            "SELECT id FROM run \
             WHERE session_id = $sid AND id != $rid AND ended_at IS NOT NONE \
             ORDER BY ended_at DESC LIMIT 1",
        )
        .bind(("sid", session_id.clone()))
        .bind(("rid", run_id.clone()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    if let Some(prev_id) = rows.into_iter().next().and_then(|r| r.id) {
        upsert_edge(db, run_id, &prev_id, "follows", "system", 1.0).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jaccard_basic() {
        let a: HashSet<&str> = ["x", "y", "z"].into_iter().collect();
        let b: HashSet<&str> = ["y", "z", "w"].into_iter().collect();
        assert!((jaccard(&a, &b) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn jaccard_disjoint_is_zero() {
        let a: HashSet<&str> = ["x"].into_iter().collect();
        let b: HashSet<&str> = ["y"].into_iter().collect();
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_both_empty_is_zero() {
        let a: HashSet<&str> = HashSet::new();
        let b: HashSet<&str> = HashSet::new();
        assert_eq!(jaccard(&a, &b), 0.0);
    }
}
