use std::collections::HashMap;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::db::Db;
use crate::embed::Embedder;
use crate::models::{RrfResult, SearchResult};

/// Full-text BM25 search across observations AND memories.
pub async fn search_text(db: &Surreal<Db>, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    // Search observations via RRF on title + narrative
    let obs_sql = format!(
        "search::rrf([\
             (SELECT id, search::score(1) AS ft_score \
              FROM observation WHERE title @1,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit}),\
             (SELECT id, search::score(2) AS ft_score \
              FROM observation WHERE narrative @2,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit})\
         ], {limit}, 60)"
    );

    let mut response = db.query(&obs_sql).bind(("q", query.to_string())).await?;
    let fused: Vec<RrfResult> = response.take(0)?;
    let mut results = fetch_observation_results(db, &fused).await?;

    // Also search memories (hifz table)
    let mem_results = search_memories(db, query, limit).await?;
    results.extend(mem_results);

    // Re-sort by score descending
    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    Ok(results)
}

/// Semantic vector search via HNSW index (observations only — memories don't have embeddings).
pub async fn search_semantic(
    db: &Surreal<Db>,
    embedder: &Embedder,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let query_vec = embedder.embed_single(query)?;
    let sql = format!(
        "SELECT id, session_id, title, obs_type, narrative, timestamp, importance, \
         vector::similarity::cosine(embedding, $query_vec) AS score \
         FROM observation WHERE embedding <|{limit},80|> $query_vec \
         ORDER BY score DESC"
    );
    let mut response = db.query(&sql).bind(("query_vec", query_vec)).await?;
    let results: Vec<SearchResult> = response.take(0)?;
    Ok(results)
}

/// Hybrid search: BM25 + vector + RRF fusion on observations, plus BM25 on memories.
pub async fn search_hybrid(
    db: &Surreal<Db>,
    embedder: &Embedder,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let query_vec = embedder.embed_single(query)?;

    // RRF fusion: vector + BM25 title + BM25 narrative (observations)
    let sql = format!(
        "search::rrf([\
             (SELECT id, vector::distance::knn() AS distance \
              FROM observation WHERE embedding <|{limit},80|> $query_vec),\
             (SELECT id, search::score(1) AS ft_score \
              FROM observation WHERE title @1,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit}),\
             (SELECT id, search::score(2) AS ft_score \
              FROM observation WHERE narrative @2,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit})\
         ], {limit}, 60)"
    );

    let mut response = db
        .query(&sql)
        .bind(("query_vec", query_vec))
        .bind(("q", query.to_string()))
        .await?;
    let fused: Vec<RrfResult> = response.take(0)?;
    let mut results = fetch_observation_results(db, &fused).await?;

    // Also search memories (hifz table) via BM25
    let mem_results = search_memories(db, query, limit).await?;
    results.extend(mem_results);

    // Re-sort and truncate
    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    // Session diversification: max 3 results per session
    diversify_by_session(&mut results, 3);
    Ok(results)
}

/// Search the hifz (long-term memories) table via BM25.
/// Maps memory fields to SearchResult fields for unified display.
async fn search_memories(db: &Surreal<Db>, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    // Search title
    let title_sql = format!(
        "SELECT id, title, content, mem_type, created_at, strength \
         FROM hifz WHERE title @1,OR@ $q \
         ORDER BY strength DESC LIMIT {limit}"
    );
    let mut resp1 = db.query(&title_sql).bind(("q", query.to_string())).await?;

    #[derive(Debug, SurrealValue)]
    struct MemRow {
        id: Option<surrealdb::types::RecordId>,
        title: Option<String>,
        content: Option<String>,
        mem_type: Option<String>,
        created_at: Option<String>,
        strength: Option<f64>,
    }

    let title_rows: Vec<MemRow> = resp1.take(0)?;

    // Search content
    let content_sql = format!(
        "SELECT id, title, content, mem_type, created_at, strength \
         FROM hifz WHERE content @2,OR@ $q \
         ORDER BY strength DESC LIMIT {limit}"
    );
    let mut resp2 = db
        .query(&content_sql)
        .bind(("q", query.to_string()))
        .await?;
    let content_rows: Vec<MemRow> = resp2.take(0)?;

    // Merge and deduplicate by id
    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();

    for row in title_rows.into_iter().chain(content_rows) {
        let id_key = format!("{:?}", row.id);
        if seen.contains(&id_key) {
            continue;
        }
        seen.insert(id_key);

        results.push(SearchResult {
            id: row.id,
            session_id: None, // memories are cross-session
            title: row.title.unwrap_or_default(),
            obs_type: format!("memory:{}", row.mem_type.unwrap_or_default()),
            narrative: row.content.unwrap_or_default(),
            timestamp: row.created_at.unwrap_or_default(),
            importance: row.strength.unwrap_or(1.0) as i64,
            score: Some(row.strength.unwrap_or(1.0)), // use strength as score
        });
    }

    Ok(results)
}

/// Fetch full observation records and attach RRF scores.
async fn fetch_observation_results(
    db: &Surreal<Db>,
    fused: &[RrfResult],
) -> Result<Vec<SearchResult>> {
    if fused.is_empty() {
        return Ok(vec![]);
    }

    let ids: Vec<surrealdb::types::RecordId> = fused.iter().filter_map(|r| r.id.clone()).collect();

    let mut fetch_resp = db
        .query(
            "SELECT id, session_id, title, obs_type, narrative, timestamp, importance, \
             0.0 AS score FROM observation WHERE id IN $ids",
        )
        .bind(("ids", ids))
        .await?;
    let mut results: Vec<SearchResult> = fetch_resp.take(0)?;

    // Attach RRF scores
    #[allow(clippy::mutable_key_type)]
    let score_map: HashMap<_, _> = fused
        .iter()
        .filter_map(|r| Some((r.id.clone()?, r.rrf_score?)))
        .collect();
    for r in &mut results {
        if let Some(ref rid) = r.id {
            r.score = score_map.get(rid).copied();
        }
    }
    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(results)
}

/// Limit results to max N per session for diversity.
fn diversify_by_session(results: &mut Vec<SearchResult>, max_per_session: usize) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    results.retain(|r| {
        let key = r
            .session_id
            .as_ref()
            .map(|s| format!("{s:?}"))
            .unwrap_or_else(|| "memory".to_string()); // memories don't have sessions
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        *count <= max_per_session
    });
}
