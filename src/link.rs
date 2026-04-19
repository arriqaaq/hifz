//! Deterministic write-time link generation between memories.
//!
//! On each new `hifz` row, we look for related existing rows via three
//! channels — embedding KNN, concept Jaccard, file Jaccard — and `RELATE`
//! them through the typed `mem_link` edge. Entity-based links (Phase 4)
//! plug into this pipeline via `via='entity'`.
//!
//! All set math (Jaccard) runs in Rust because SurrealQL has no native
//! intersection/union operators (verified against the hadith codebase and
//! SurrealDB test fixtures).

use std::collections::HashSet;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

/// Cosine-distance upper bound (= 1 − similarity lower bound) for `via='embedding'`.
const EMBEDDING_DISTANCE_MAX: f64 = 0.25;
/// Jaccard lower bound for `via='concept' | 'file'`.
const JACCARD_MIN: f64 = 0.30;
/// HNSW KNN fanout used at write time.
const KNN_K: usize = 10;
const KNN_EF: usize = 100;

#[derive(Debug, SurrealValue)]
struct CandidateRow {
    id: Option<RecordId>,
    distance: Option<f64>,
    concepts: Option<Vec<String>>,
    files: Option<Vec<String>>,
}

#[derive(Debug, Default)]
pub struct LinkReport {
    pub embedding_links: usize,
    pub concept_links: usize,
    pub file_links: usize,
}

/// Generate links for a freshly-written memory. `self_id` must be the record
/// id of the new row (e.g. `hifz:xyz`); `concepts` and `files` are its own.
pub async fn generate_links(
    db: &Surreal<Db>,
    self_id: &RecordId,
    project: &str,
    embedding: &[f32],
    concepts: &[String],
    files: &[String],
) -> Result<LinkReport> {
    let mut report = LinkReport::default();

    // KNN sweep over existing memories in the same project (or global), excluding self.
    let sql = format!(
        "SELECT id, vector::distance::knn() AS distance, concepts, files \
         FROM hifz \
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

    let self_concepts: HashSet<&str> = concepts.iter().map(String::as_str).collect();
    let self_files: HashSet<&str> = files.iter().map(String::as_str).collect();

    for c in &candidates {
        let Some(other_id) = c.id.clone() else {
            continue;
        };

        // Embedding link
        if let Some(d) = c.distance {
            if d < EMBEDDING_DISTANCE_MAX {
                let score = (1.0 - d).clamp(0.0, 1.0);
                upsert_link(db, self_id, &other_id, "embedding", score).await?;
                report.embedding_links += 1;
            }
        }

        // Concept link
        if !self_concepts.is_empty() {
            let other: HashSet<&str> = c
                .concepts
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let j = jaccard(&self_concepts, &other);
            if j >= JACCARD_MIN {
                upsert_link(db, self_id, &other_id, "concept", j).await?;
                report.concept_links += 1;
            }
        }

        // File link
        if !self_files.is_empty() {
            let other: HashSet<&str> = c
                .files
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let j = jaccard(&self_files, &other);
            if j >= JACCARD_MIN {
                upsert_link(db, self_id, &other_id, "file", j).await?;
                report.file_links += 1;
            }
        }
    }

    Ok(report)
}

/// Upsert a single link with per-`via` dedup and `math::max`-style score merge.
///
/// SurrealDB's `RELATE UNIQUE` enforces `(in, out)` uniqueness only (verified
/// in surrealdb/core/src/syn/parser/test/stmt.rs:121), so per-via dedup must
/// happen here. Check → update-or-create → done.
pub async fn upsert_link(
    db: &Surreal<Db>,
    from: &RecordId,
    to: &RecordId,
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
            "SELECT id, score FROM mem_link \
             WHERE in = $from AND out = $to AND via = $via \
             LIMIT 1",
        )
        .bind(("from", from.clone()))
        .bind(("to", to.clone()))
        .bind(("via", via.to_string()))
        .await?;
    let existing: Vec<Existing> = resp.take(0).unwrap_or_default();

    if let Some(row) = existing.into_iter().next() {
        if let Some(id) = row.id {
            let old = row.score.unwrap_or(0.0);
            if score > old {
                db.query("UPDATE type::thing($id) SET score = $score")
                    .bind(("id", id))
                    .bind(("score", score))
                    .await?
                    .check()?;
            }
        }
        return Ok(());
    }

    let now = chrono::Utc::now().to_rfc3339();
    db.query("RELATE $from->mem_link->$to SET score = $score, via = $via, created_at = $now")
        .bind(("from", from.clone()))
        .bind(("to", to.clone()))
        .bind(("score", score))
        .bind(("via", via.to_string()))
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
// 1-hop graph expansion at retrieval time
// ---------------------------------------------------------------------------

/// A single edge row returned by `expand_neighbours`.
#[derive(Debug, Clone)]
pub struct EdgeHit {
    pub from: RecordId,
    pub to: RecordId,
    pub score: f64,
    pub via: String,
}

/// Fetch outgoing `mem_link` edges for the given seed memory ids.
///
/// IMPORTANT: the `SELECT ->edge->node.*` form does **not** return edge fields
/// (verified from hadith/src/analysis/isnad_graph.rs:206-207), so we query the
/// edge table directly and the caller joins neighbour rows in Rust.
pub async fn expand_neighbours(db: &Surreal<Db>, seed_ids: &[RecordId]) -> Result<Vec<EdgeHit>> {
    if seed_ids.is_empty() {
        return Ok(vec![]);
    }

    #[derive(Debug, SurrealValue)]
    struct Row {
        #[surreal(rename = "in")]
        in_: Option<RecordId>,
        out: Option<RecordId>,
        score: Option<f64>,
        via: Option<String>,
    }

    let mut resp = db
        .query("SELECT in, out, score, via FROM mem_link WHERE in IN $ids")
        .bind(("ids", seed_ids.to_vec()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(EdgeHit {
                from: r.in_?,
                to: r.out?,
                score: r.score.unwrap_or(0.0),
                via: r.via.unwrap_or_default(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jaccard_basic() {
        let a: HashSet<&str> = ["x", "y", "z"].into_iter().collect();
        let b: HashSet<&str> = ["y", "z", "w"].into_iter().collect();
        // intersection = {y, z} = 2; union = {x, y, z, w} = 4; 2/4 = 0.5
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
