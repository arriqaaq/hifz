use std::collections::HashMap;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::db::Db;
use crate::embed::Embedder;
use crate::link;
use crate::models::{RrfResult, SearchResult};
use crate::rank;
use crate::rerank::Reranker;

/// Ablation / tuning knobs for the hybrid retrieval path. `Default` preserves
/// the normal Phase-1–6 behaviour; each `bool` turns one stage OFF so the bench
/// (and future callers) can measure that stage's contribution.
#[derive(Debug, Clone, Copy)]
pub struct SearchConfig {
    /// Skip the vector branch in memory RRF (falls back to BM25-only for memories).
    pub skip_vector: bool,
    /// Skip the `strength · recency · access` Rust-side re-ranking (RRF scores passed through).
    pub skip_recency_access: bool,
    /// Skip 1-hop `memory_link` expansion.
    pub skip_graph: bool,
    /// Skip the session/identity-based diversification pass entirely.
    pub skip_diversify: bool,
    /// RRF constant `k` for `search::rrf([...], limit, k)`. Smaller values
    /// sharpen the gap between ranks (1/(k+rank) decays faster). Default 60
    /// matches the widely-cited literature baseline; lowering to ~20 trades
    /// robustness-to-noise for clearer separation when multiple weak matches
    /// compete (verified via Phase 8.5 bench to be the dominant cause of
    /// close-competitor misses).
    pub rrf_k: u64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            skip_vector: false,
            skip_recency_access: false,
            skip_graph: false,
            skip_diversify: false,
            // Phase 9.1: default lowered from the TREC-literature 60 to 10
            // based on the `memory-bench` sweep — at k=60 the small-corpus
            // regime dilutes cross-branch ordering (Recall@5 = 0.922), at
            // k=10 it's 0.944 with no regressions. k=5 was strictly better
            // still on the synthetic fixture but we hold that back for real
            // data to avoid overfitting. Override via `SearchConfig::rrf_k`
            // for users who want to sweep.
            rrf_k: 10,
        }
    }
}

/// Full-text BM25 search across observations AND memories.
pub async fn search_text(
    db: &Surreal<Db>,
    query: &str,
    limit: usize,
    project: Option<&str>,
) -> Result<Vec<SearchResult>> {
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

    // Also search memories (memory table)
    let mem_results = search_memories(db, None, query, limit, project).await?;
    results.extend(mem_results);

    // Re-sort by score descending
    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    bump_memory_access(db, &results).await;
    Ok(results)
}

/// Semantic vector search via HNSW index (observations only for now).
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

/// Hybrid search: BM25 + vector + RRF fusion on observations, hybrid on memories.
/// Preserves the pre-Phase-7 behaviour.
pub async fn search_hybrid(
    db: &Surreal<Db>,
    embedder: &Embedder,
    query: &str,
    limit: usize,
    project: Option<&str>,
) -> Result<Vec<SearchResult>> {
    search_hybrid_with_config(db, embedder, query, limit, project, SearchConfig::default()).await
}

/// Hybrid search with per-stage ablation knobs. The default [`SearchConfig`]
/// matches [`search_hybrid`] exactly — turning fields on disables the named
/// stage so the bench can attribute recall to each stage.
pub async fn search_hybrid_with_config(
    db: &Surreal<Db>,
    embedder: &Embedder,
    query: &str,
    limit: usize,
    project: Option<&str>,
    cfg: SearchConfig,
) -> Result<Vec<SearchResult>> {
    let query_vec = embedder.embed_single(query)?;

    // RRF fusion: vector + BM25 title + BM25 narrative (observations)
    let rrf_k = cfg.rrf_k;
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
         ], {limit}, {rrf_k})"
    );

    let mut response = db
        .query(&sql)
        .bind(("query_vec", query_vec.clone()))
        .bind(("q", query.to_string()))
        .await?;
    let fused: Vec<RrfResult> = response.take(0)?;
    let mut results = fetch_observation_results(db, &fused).await?;

    // Hybrid memory search (vector + BM25 title + BM25 content, ablation-aware)
    let query_vec_for_mem = if cfg.skip_vector {
        None
    } else {
        Some(&query_vec)
    };
    let mem_results =
        search_memories_with_config(db, query_vec_for_mem, query, limit, project, cfg).await?;
    results.extend(mem_results);

    // Run search (BM25 on lesson + prompt, score dampened by 0.7)
    let run_results = search_runs_for_context(db, query, limit / 2, project).await?;
    results.extend(run_results);

    // Re-sort and truncate before graph expansion — we only expand from the top-K
    // seeds, not every candidate, to keep the traversal cheap.
    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);

    if !cfg.skip_graph {
        expand_from_graph(db, &mut results, limit).await;
    }

    if !cfg.skip_diversify {
        diversify_by_session(&mut results, 3);
    }

    bump_memory_access(db, &results).await;
    Ok(results)
}

/// 1-hop graph expansion in-place. Seeds are existing memory results; their
/// outgoing memory_link edges pull in neighbours that may not have hit the vector
/// or BM25 branches directly but are graph-close to something that did.
async fn expand_from_graph(db: &Surreal<Db>, results: &mut Vec<SearchResult>, limit: usize) {
    let seed_by_id: HashMap<surrealdb::types::RecordId, f64> = results
        .iter()
        .filter(|r| r.obs_type.starts_with("memory:"))
        .filter_map(|r| Some((r.id.clone()?, r.score.unwrap_or(0.0))))
        .collect();
    if seed_by_id.is_empty() {
        return;
    }

    let seed_ids: Vec<_> = seed_by_id.keys().cloned().collect();
    let edges = match link::expand_neighbours(db, &seed_ids).await {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!("graph expansion failed: {err}");
            return;
        }
    };
    if edges.is_empty() {
        return;
    }

    // Collect neighbour ids we haven't already surfaced.
    #[allow(clippy::mutable_key_type)]
    let already: std::collections::HashSet<_> =
        results.iter().filter_map(|r| r.id.clone()).collect();
    let neighbour_ids: Vec<_> = edges
        .iter()
        .filter(|e| !already.contains(&e.to))
        .map(|e| e.to.clone())
        .collect();
    if neighbour_ids.is_empty() {
        return;
    }

    // Fetch neighbour rows in one query, join with edges in Rust.
    #[derive(Debug, SurrealValue)]
    struct NRow {
        id: Option<surrealdb::types::RecordId>,
        title: Option<String>,
        content: Option<String>,
        category: Option<String>,
        created_at: Option<String>,
        strength: Option<f64>,
        retrieval_count: Option<i64>,
    }
    let mut resp = match db
        .query(
            "SELECT id, title, content, category, created_at, strength, retrieval_count \
             FROM memory \
             WHERE id IN $ids AND is_latest = true",
        )
        .bind(("ids", neighbour_ids))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("graph neighbour fetch failed: {e}");
            return;
        }
    };
    let rows: Vec<NRow> = resp.take(0).unwrap_or_default();

    #[allow(clippy::mutable_key_type)]
    let row_by_id: HashMap<_, _> = rows
        .into_iter()
        .filter_map(|r| r.id.clone().map(|id| (id, r)))
        .collect();

    // Combine: neighbour_score = seed_score * 0.5 * edge.score (dampened hop).
    // Keep the best combined score if multiple edges reach the same neighbour.
    #[allow(clippy::mutable_key_type)]
    let mut best_for_neighbour: HashMap<surrealdb::types::RecordId, (f64, String)> = HashMap::new();
    for e in &edges {
        if already.contains(&e.to) {
            continue;
        }
        let Some(seed_score) = seed_by_id.get(&e.from) else {
            continue;
        };
        let combined = seed_score * 0.5 * e.score;
        let entry = best_for_neighbour
            .entry(e.to.clone())
            .or_insert((combined, e.via.clone()));
        if combined > entry.0 {
            *entry = (combined, e.via.clone());
        }
    }

    for (nid, (score, via)) in best_for_neighbour {
        let Some(row) = row_by_id.get(&nid) else {
            continue;
        };
        let strength = row.strength.unwrap_or(1.0);
        let created = row.created_at.clone().unwrap_or_default();
        let access = row.retrieval_count.unwrap_or(0);
        let final_score = rank::final_score(score * strength, &created, access);
        results.push(SearchResult {
            id: row.id.clone(),
            session_id: None,
            title: row.title.clone().unwrap_or_default(),
            obs_type: format!(
                "memory:{}@via:{via}",
                row.category.clone().unwrap_or_default()
            ),
            narrative: row.content.clone().unwrap_or_default(),
            timestamp: created,
            importance: (strength * 10.0) as i64,
            score: Some(final_score),
            is_neighbor: false,
        });
    }

    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
}

/// Hybrid search across the `memory` (long-term memory) table (default config).
async fn search_memories(
    db: &Surreal<Db>,
    query_vec: Option<&Vec<f32>>,
    query: &str,
    limit: usize,
    project: Option<&str>,
) -> Result<Vec<SearchResult>> {
    search_memories_with_config(
        db,
        query_vec,
        query,
        limit,
        project,
        SearchConfig::default(),
    )
    .await
}

/// Hybrid search across `memory` with ablation support.
///
/// - If `query_vec` is provided, RRF-fuses vector + BM25(title) + BM25(content).
/// - Otherwise falls back to BM25(title) + BM25(content) fusion.
/// - Always project-scopes when `project` is provided.
/// - Applies Rust-side `strength · recency · access` boost to the fused score,
///   unless `cfg.skip_recency_access` is set (bench ablation).
async fn search_memories_with_config(
    db: &Surreal<Db>,
    query_vec: Option<&Vec<f32>>,
    query: &str,
    limit: usize,
    project: Option<&str>,
    cfg: SearchConfig,
) -> Result<Vec<SearchResult>> {
    #[derive(Debug, SurrealValue)]
    struct MemRow {
        id: Option<surrealdb::types::RecordId>,
        title: Option<String>,
        content: Option<String>,
        category: Option<String>,
        created_at: Option<String>,
        strength: Option<f64>,
        retrieval_count: Option<i64>,
    }

    let project_filter = if project.is_some() {
        " AND project = $project"
    } else {
        ""
    };

    // Build RRF fusion branches. Vector branch only if we have an embedding.
    let vector_branch = if query_vec.is_some() {
        format!(
            "(SELECT id, vector::distance::knn() AS distance \
              FROM memory WHERE is_latest = true{project_filter} \
              AND embedding <|{limit},80|> $query_vec),"
        )
    } else {
        String::new()
    };

    let rrf_k = cfg.rrf_k;
    let sql = format!(
        "search::rrf([\
             {vector_branch}\
             (SELECT id, search::score(1) AS ft_score \
              FROM memory WHERE is_latest = true{project_filter} AND title @1,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit}),\
             (SELECT id, search::score(2) AS ft_score \
              FROM memory WHERE is_latest = true{project_filter} AND content @2,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit})\
         ], {limit}, {rrf_k})"
    );

    let mut q = db.query(&sql).bind(("q", query.to_string()));
    if let Some(v) = query_vec {
        q = q.bind(("query_vec", v.clone()));
    }
    if let Some(p) = project {
        q = q.bind(("project", p.to_string()));
    }
    let mut response = q.await?;
    let fused: Vec<RrfResult> = response.take(0)?;
    if fused.is_empty() {
        return Ok(vec![]);
    }

    let ids: Vec<surrealdb::types::RecordId> = fused.iter().filter_map(|r| r.id.clone()).collect();
    let mut fetch = db
        .query(
            "SELECT id, title, content, category, created_at, strength, retrieval_count \
             FROM memory WHERE id IN $ids",
        )
        .bind(("ids", ids))
        .await?;
    let rows: Vec<MemRow> = fetch.take(0)?;

    // Attach RRF scores, boost by strength · recency · access.
    #[allow(clippy::mutable_key_type)]
    let rrf_map: HashMap<_, _> = fused
        .iter()
        .filter_map(|r| Some((r.id.clone()?, r.rrf_score?)))
        .collect();

    let mut results: Vec<SearchResult> = rows
        .into_iter()
        .map(|row| {
            let id = row.id.clone();
            let rrf = id
                .as_ref()
                .and_then(|r| rrf_map.get(r).copied())
                .unwrap_or(0.0);
            let strength = row.strength.unwrap_or(1.0);
            let created_at = row.created_at.clone().unwrap_or_default();
            let access = row.retrieval_count.unwrap_or(0);
            let score = if cfg.skip_recency_access {
                rrf * strength
            } else {
                rank::final_score(rrf * strength, &created_at, access)
            };

            SearchResult {
                id,
                session_id: None,
                title: row.title.unwrap_or_default(),
                obs_type: format!("memory:{}", row.category.unwrap_or_default()),
                narrative: row.content.unwrap_or_default(),
                timestamp: created_at,
                importance: (strength * 10.0) as i64,
                score: Some(score),
                is_neighbor: false,
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    Ok(results)
}

/// Re-score the top-`top_n` memory rows in `results` with a fastembed
/// cross-encoder reranker. Observation rows are left untouched. The full
/// list is re-sorted after reranking so memories may move above/below
/// observations based on the new scores.
///
/// The reranker's score *replaces* the first-stage score for the memory
/// rows it touches. We don't blend — mixing dilutes the cross-attention
/// signal. Memory rows beyond `top_n` keep their first-stage score.
///
/// Errors degrade gracefully: the original `results` are returned
/// unchanged and the error is logged via `tracing::warn!`.
pub fn apply_rerank(
    reranker: &Reranker,
    query: &str,
    mut results: Vec<SearchResult>,
    top_n: usize,
) -> Vec<SearchResult> {
    let mem_indices: Vec<usize> = results
        .iter()
        .enumerate()
        .filter(|(_, r)| r.obs_type.starts_with("memory:"))
        .map(|(i, _)| i)
        .take(top_n)
        .collect();
    if mem_indices.is_empty() {
        return results;
    }

    let docs: Vec<String> = mem_indices
        .iter()
        .map(|&i| {
            let r = &results[i];
            if r.narrative.is_empty() {
                r.title.clone()
            } else {
                format!("{}\n{}", r.title, r.narrative)
            }
        })
        .collect();

    match reranker.rerank(query, &docs) {
        Ok(scored) => {
            for (docs_idx, new_score) in scored {
                if let Some(&r_idx) = mem_indices.get(docs_idx) {
                    results[r_idx].score = Some(new_score);
                }
            }
            results.sort_by(|a, b| {
                b.score
                    .unwrap_or(0.0)
                    .partial_cmp(&a.score.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        Err(e) => {
            tracing::warn!("reranker failed, returning first-stage order: {e}");
        }
    }
    results
}

/// Increment `retrieval_count` and bump `last_accessed_at` for any memory hits.
/// Fire-and-forget; errors are logged but not returned.
async fn bump_memory_access(db: &Surreal<Db>, results: &[SearchResult]) {
    let mem_ids: Vec<surrealdb::types::RecordId> = results
        .iter()
        .filter(|r| r.obs_type.starts_with("memory:"))
        .filter_map(|r| r.id.clone())
        .collect();
    if mem_ids.is_empty() {
        return;
    }
    let q =
        "UPDATE memory SET retrieval_count += 1, last_accessed_at = time::now() WHERE id IN $ids";
    if let Err(e) = db.query(q).bind(("ids", mem_ids)).await {
        tracing::warn!("access bump failed: {e}");
    }
}

/// Search closed runs with lessons for context integration.
/// Runs with lessons are surfaced as SearchResults with obs_type = "run:task".
/// Scores are dampened by 0.7 (runs are contextual, not authoritative).
pub async fn search_runs_for_context(
    db: &Surreal<Db>,
    query: &str,
    limit: usize,
    project: Option<&str>,
) -> Result<Vec<SearchResult>> {
    #[derive(Debug, SurrealValue)]
    struct RunRow {
        id: Option<surrealdb::types::RecordId>,
        prompt: Option<String>,
        lesson: Option<String>,
        outcome: Option<String>,
        commit_id: Option<surrealdb::types::RecordId>,
        ended_at: Option<String>,
        session_id: Option<surrealdb::types::RecordId>,
    }

    let project_filter = if project.is_some() {
        " AND project = $project"
    } else {
        ""
    };

    // BM25 search on lesson + prompt
    let rrf_k = 60;
    let sql = format!(
        "search::rrf([\
             (SELECT id, search::score(1) AS ft_score \
              FROM run WHERE ended_at IS NOT NONE \
                AND lesson IS NOT NONE AND lesson != ''{project_filter} \
                AND prompt @1,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit}),\
             (SELECT id, search::score(2) AS ft_score \
              FROM run WHERE ended_at IS NOT NONE \
                AND lesson IS NOT NONE AND lesson != ''{project_filter} \
                AND lesson @2,OR@ $q \
              ORDER BY ft_score DESC LIMIT {limit})\
         ], {limit}, {rrf_k})"
    );

    let mut q = db.query(&sql).bind(("q", query.to_string()));
    if let Some(p) = project {
        q = q.bind(("project", p.to_string()));
    }
    let mut response = q.await?;
    let fused: Vec<RrfResult> = response.take(0)?;
    if fused.is_empty() {
        return Ok(vec![]);
    }

    let ids: Vec<surrealdb::types::RecordId> = fused.iter().filter_map(|r| r.id.clone()).collect();
    let mut fetch = db
        .query(
            "SELECT id, prompt, lesson, outcome, commit_id, ended_at, session_id \
             FROM run WHERE id IN $ids",
        )
        .bind(("ids", ids))
        .await?;
    let rows: Vec<RunRow> = fetch.take(0)?;

    #[allow(clippy::mutable_key_type)]
    let rrf_map: HashMap<_, _> = fused
        .iter()
        .filter_map(|r| Some((r.id.clone()?, r.rrf_score?)))
        .collect();

    let results: Vec<SearchResult> = rows
        .into_iter()
        .map(|row| {
            let id = row.id.clone();
            let rrf = id
                .as_ref()
                .and_then(|r| rrf_map.get(r).copied())
                .unwrap_or(0.0);
            // Dampen run scores by 0.7 (contextual, not authoritative)
            let score = rrf * 0.7;

            let title = row.prompt.clone().unwrap_or_default();
            let narrative = row.lesson.clone().unwrap_or_default();
            let outcome = row.outcome.clone().unwrap_or_default();

            SearchResult {
                id,
                session_id: row.session_id.clone(),
                title: truncate_str(&title, 80),
                obs_type: format!("run:task:{outcome}"),
                narrative,
                timestamp: row.ended_at.unwrap_or_default(),
                importance: 5,
                score: Some(score),
                is_neighbor: false,
            }
        })
        .collect();

    Ok(results)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
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
///
/// Observations (which carry a `session_id`) are capped at `max_per_session` per
/// session. Memory rows have no session — they are keyed by their own id so
/// each memory is its own diversity class (the cap becomes a no-op for them).
/// Earlier versions keyed all memories with the literal string `"memory"`,
/// silently truncating every query's memory pool to `max_per_session` total —
/// see Phase 7a.
fn diversify_by_session(results: &mut Vec<SearchResult>, max_per_session: usize) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    results.retain(|r| {
        let key = match r.session_id.as_ref() {
            Some(s) => format!("session:{s:?}"),
            None => match r.id.as_ref() {
                Some(id) => format!("mem:{id:?}"),
                // No session, no id — treat as unique so it isn't falsely bucketed.
                None => format!("anon:{}", counts.len()),
            },
        };
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        *count <= max_per_session
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_memory(title: &str, idx: u64) -> SearchResult {
        SearchResult {
            id: Some(surrealdb::types::RecordId::new("memory", format!("m{idx}"))),
            session_id: None,
            title: title.to_string(),
            obs_type: "memory:fact".to_string(),
            narrative: String::new(),
            timestamp: String::new(),
            importance: 5,
            score: Some(1.0 - idx as f64 * 0.01),
            is_neighbor: false,
        }
    }

    #[test]
    fn diversification_does_not_collapse_memories() {
        // Regression test for Phase 7a. Five distinct memories (no session_id)
        // must all survive a cap of 3, because each memory is its own
        // diversity class.
        let mut results: Vec<SearchResult> =
            (0..5).map(|i| mk_memory(&format!("m{i}"), i)).collect();
        diversify_by_session(&mut results, 3);
        assert_eq!(results.len(), 5, "memories were erroneously capped");
    }
}
