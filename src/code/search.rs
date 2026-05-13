//! Hybrid (vector + BM25) search over `code_chunk`.
//!
//! Modeled directly on `crate::search::search_memory_chunks`. Returns
//! per-chunk hits with line ranges and snippets. Optional `group_by_file`
//! collapses to one hit per file (max-score chunk wins).

use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::embed::Embedder;

#[derive(Debug, Clone, Serialize)]
pub struct CodeSearchResult {
    pub id: String,
    pub file_id: String,
    pub path: String,
    pub language: String,
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
    pub score: f64,
    pub via: &'static str,
}

#[derive(Debug, Clone)]
pub struct CodeSearchOpts {
    pub limit: usize,
    pub project: Option<String>,
    pub language: Option<String>,
    /// Substring filter against `code_chunk.path`. (Glob support is M6+.)
    pub path: Option<String>,
    pub group_by_file: bool,
}

impl Default for CodeSearchOpts {
    fn default() -> Self {
        Self {
            limit: 10,
            project: None,
            language: None,
            path: None,
            group_by_file: false,
        }
    }
}

#[derive(Debug, SurrealValue)]
struct ChunkHit {
    id: Option<RecordId>,
    file: Option<RecordId>,
    path: Option<String>,
    language: Option<String>,
    content: Option<String>,
    start_line: Option<i64>,
    end_line: Option<i64>,
    distance: Option<f64>,
    ft_score: Option<f64>,
}

pub async fn search_code(
    db: &Surreal<Db>,
    embedder: &Embedder,
    query: &str,
    opts: &CodeSearchOpts,
) -> Result<Vec<CodeSearchResult>> {
    let limit = opts.limit.max(1);
    let query_vec = embedder.embed_single(query)?;

    // Build WHERE clauses dynamically.
    let mut where_extra = String::new();
    if opts.project.is_some() {
        where_extra.push_str(" AND (project = $project OR project = 'global')");
    }
    if opts.language.is_some() {
        where_extra.push_str(" AND language = $language");
    }
    if opts.path.is_some() {
        where_extra.push_str(" AND string::contains(path, $path)");
    }

    // Vector branch
    let vec_sql = format!(
        "SELECT id, file, path, language, content, start_line, end_line, \
                vector::distance::knn() AS distance \
         FROM code_chunk \
         WHERE embedding <|{limit},80|> $query_vec{where_extra}"
    );
    let mut vec_q = db.query(&vec_sql).bind(("query_vec", query_vec.to_vec()));
    if let Some(p) = &opts.project {
        vec_q = vec_q.bind(("project", p.clone()));
    }
    if let Some(l) = &opts.language {
        vec_q = vec_q.bind(("language", l.clone()));
    }
    if let Some(p) = &opts.path {
        vec_q = vec_q.bind(("path", p.clone()));
    }
    let mut vec_resp = vec_q.await?;
    let vec_hits: Vec<ChunkHit> = vec_resp.take(0).unwrap_or_default();

    // BM25 branch
    let ft_sql = format!(
        "SELECT id, file, path, language, content, start_line, end_line, \
                search::score(1) AS ft_score \
         FROM code_chunk \
         WHERE content @1@ $q{where_extra} \
         ORDER BY ft_score DESC LIMIT {limit}"
    );
    let mut ft_q = db.query(&ft_sql).bind(("q", query.to_string()));
    if let Some(p) = &opts.project {
        ft_q = ft_q.bind(("project", p.clone()));
    }
    if let Some(l) = &opts.language {
        ft_q = ft_q.bind(("language", l.clone()));
    }
    if let Some(p) = &opts.path {
        ft_q = ft_q.bind(("path", p.clone()));
    }
    let mut ft_resp = ft_q.await?;
    let ft_hits: Vec<ChunkHit> = ft_resp.take(0).unwrap_or_default();

    // Merge by chunk id; score = max(vector_sim, bm25/4) per source.
    #[derive(Debug)]
    struct Merged {
        rec: ChunkHit,
        score: f64,
        via: &'static str,
    }
    let mut by_id: HashMap<String, Merged> = HashMap::new();
    let intake = |hits: Vec<ChunkHit>, via: &'static str| {
        hits.into_iter().filter_map(move |h| {
            let id = h.id.as_ref()?;
            let key = format!("{id:?}");
            let score = h
                .distance
                .map(|d| (1.0 - d).clamp(0.0, 1.0))
                .or_else(|| h.ft_score.map(|s| (s / 4.0).clamp(0.0, 1.0)))
                .unwrap_or(0.0);
            Some((key, Merged { rec: h, score, via }))
        })
    };
    for (key, m) in intake(vec_hits, "vector") {
        by_id.entry(key).or_insert(m);
    }
    for (key, m) in intake(ft_hits, "bm25") {
        by_id
            .entry(key)
            .and_modify(|prior| {
                if m.score > prior.score {
                    prior.score = m.score;
                    prior.via = "hybrid";
                }
            })
            .or_insert(m);
    }

    let mut out: Vec<CodeSearchResult> = by_id
        .into_values()
        .filter_map(|m| {
            let id = m.rec.id?;
            let file = m.rec.file?;
            Some(CodeSearchResult {
                id: format!("{id:?}"),
                file_id: format!("{file:?}"),
                path: m.rec.path.unwrap_or_default(),
                language: m.rec.language.unwrap_or_default(),
                start_line: m.rec.start_line.unwrap_or(0).max(0) as usize,
                end_line: m.rec.end_line.unwrap_or(0).max(0) as usize,
                snippet: m.rec.content.unwrap_or_default(),
                score: m.score,
                via: m.via,
            })
        })
        .collect();

    if opts.group_by_file {
        let mut best: HashMap<String, CodeSearchResult> = HashMap::new();
        for r in out {
            best.entry(r.file_id.clone())
                .and_modify(|prior| {
                    if r.score > prior.score {
                        *prior = r.clone();
                    }
                })
                .or_insert(r);
        }
        out = best.into_values().collect();
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(limit);
    Ok(out)
}
