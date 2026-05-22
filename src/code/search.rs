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
    /// Substring filter against `code_chunk.path`. (Glob support comes later.)
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
                id: crate::rid_to_string(&id),
                file_id: crate::rid_to_string(&file),
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

/// One symbol match — the lean shape behind the agent `code/symbols` endpoint
/// (`hifz_code_search` MCP tool). No record id, no body: just enough to jump to
/// the definition. Costs ~1/10th the tokens of a `CodeSearchResult` chunk hit.
#[derive(Debug, Clone, Serialize)]
pub struct CodeSymbolHit {
    pub name: String,
    pub qualified: String,
    pub kind: String,
    pub language: String,
    pub path: String,
    /// 1-indexed line of the symbol's definition (`code_symbol.start_line`).
    pub line: usize,
}

#[derive(Debug, SurrealValue)]
struct SymbolRow {
    name: Option<String>,
    qualified: Option<String>,
    kind: Option<String>,
    language: Option<String>,
    path: Option<String>,
    start_line: Option<i64>,
}

/// Lexical symbol lookup over `code_symbol` — no embedding. Matches exact
/// `name`, exact `qualified`, or qualified-substring (uses the
/// `code_symbol_name` index for the common bare-name case). Exact name/qualified
/// hits are sorted first. This is the "find a function I can name" path: a flat,
/// id-free, body-free array, ranking the definition itself rather than the
/// chunks that mention it.
pub async fn search_symbols(
    db: &Surreal<Db>,
    project: &str,
    query: &str,
    limit: usize,
    kind: Option<&str>,
) -> Result<Vec<CodeSymbolHit>> {
    let limit = limit.max(1);
    let mut sql = String::from(
        "SELECT name, qualified, kind, language, path, start_line FROM code_symbol \
         WHERE (project = $p OR project = 'global') \
         AND (name = $q OR qualified = $q OR string::contains(qualified, $q))",
    );
    if kind.is_some() {
        sql.push_str(" AND kind = $kind");
    }
    // Over-fetch so exact-name hits survive the post-sort truncation even when
    // the DB returns substring matches first.
    sql.push_str(" LIMIT $lim");
    let mut q = db
        .query(&sql)
        .bind(("p", project.to_string()))
        .bind(("q", query.to_string()))
        .bind(("lim", (limit * 4) as i64));
    if let Some(k) = kind {
        q = q.bind(("kind", k.to_string()));
    }
    let mut resp = q.await?;
    let rows: Vec<SymbolRow> = resp.take(0).unwrap_or_default();
    let mut hits: Vec<CodeSymbolHit> = rows
        .into_iter()
        .map(|r| CodeSymbolHit {
            name: r.name.unwrap_or_default(),
            qualified: r.qualified.unwrap_or_default(),
            kind: r.kind.unwrap_or_default(),
            language: r.language.unwrap_or_default(),
            path: r.path.unwrap_or_default(),
            line: r.start_line.unwrap_or(0).max(0) as usize,
        })
        .collect();
    // Exact name/qualified matches first; stable within each bucket.
    hits.sort_by_key(|h| {
        u8::from(!(h.name.eq_ignore_ascii_case(query) || h.qualified.eq_ignore_ascii_case(query)))
    });
    hits.truncate(limit);
    Ok(hits)
}

/// Lean chunk hit for the agent `code/semantic` endpoint (`hifz_code_semantic`
/// MCP tool): path + line range + a truncated snippet, nothing else. The agent
/// reads `path:start_line` for the full body if it needs more.
/// One semantic hit — a ranked pointer, not a body. When the matched chunk sits
/// inside a known definition we report that symbol's whole-line `signature` and
/// def line (no truncation, no mid-statement slicing); otherwise (imports,
/// comments, markdown/config, `Plain` files — ~40% of chunks) we return a bare
/// `path:line` pointer and let the caller Read on demand. Mirrors how Claude
/// Code searches: a trustworthy location plus just enough to decide to Read.
#[derive(Debug, Clone, Serialize)]
pub struct CodeSemanticHit {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, SurrealValue)]
struct EnclosingSymbol {
    qualified: Option<String>,
    kind: Option<String>,
    signature: Option<String>,
    start_line: Option<i64>,
    end_line: Option<i64>,
}

/// Symbols whose **definition starts inside** the chunk's line span `[cs, ce]`,
/// in `path`. This names what the matched region actually *defines* — so a chunk
/// covering two adjacent functions yields both, and the enclosing `impl`/`struct`
/// (whose def starts before the chunk) is excluded. Capped to avoid one dense
/// chunk flooding the result. Uses the `code_symbol_path` index.
async fn symbols_defined_in(
    db: &Surreal<Db>,
    project: &str,
    path: &str,
    cs: usize,
    ce: usize,
) -> Vec<EnclosingSymbol> {
    let mut resp = match db
        .query(
            "SELECT qualified, kind, signature, start_line, end_line FROM code_symbol \
             WHERE (project = $p OR project = 'global') AND path = $path \
             AND start_line >= $cs AND start_line <= $ce \
             ORDER BY start_line LIMIT 4",
        )
        .bind(("p", project.to_string()))
        .bind(("path", path.to_string()))
        .bind(("cs", cs as i64))
        .bind(("ce", ce as i64))
        .await
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    resp.take(0).unwrap_or_default()
}

/// Innermost `code_symbol` whose range *contains* `line` — fallback for a chunk
/// that sits in the middle of a large body and defines nothing of its own.
/// `ORDER BY start_line DESC` picks the tightest (a method over its impl).
async fn enclosing_symbol(
    db: &Surreal<Db>,
    project: &str,
    path: &str,
    line: usize,
) -> Option<EnclosingSymbol> {
    let mut resp = db
        .query(
            "SELECT qualified, kind, signature, start_line, end_line FROM code_symbol \
             WHERE (project = $p OR project = 'global') AND path = $path \
             AND start_line <= $line AND end_line >= $line \
             ORDER BY start_line DESC LIMIT 1",
        )
        .bind(("p", project.to_string()))
        .bind(("path", path.to_string()))
        .bind(("line", line as i64))
        .await
        .ok()?;
    let rows: Vec<EnclosingSymbol> = resp.take(0).unwrap_or_default();
    rows.into_iter().next()
}

impl EnclosingSymbol {
    fn into_hit(self, path: &str, score: f64, chunk: (usize, usize)) -> CodeSemanticHit {
        CodeSemanticHit {
            path: path.to_string(),
            start_line: self.start_line.unwrap_or(chunk.0 as i64).max(0) as usize,
            end_line: self.end_line.unwrap_or(chunk.1 as i64).max(0) as usize,
            score,
            qualified: self.qualified,
            kind: self.kind,
            signature: self.signature,
        }
    }
}

/// Semantic code search returning symbol-anchored pointers (no chunk bodies).
/// For each hybrid `search_code` hit (over-fetched): report the symbols *defined
/// within* the matched chunk; if it defines none, the enclosing symbol; if it's
/// in a non-code/non-symbol region, a bare `path:line` pointer. Dedupes by
/// `qualified` (else overlapping line range) and truncates to `opts.limit`.
pub async fn search_semantic(
    db: &Surreal<Db>,
    embedder: &Embedder,
    query: &str,
    opts: &CodeSearchOpts,
) -> Result<Vec<CodeSemanticHit>> {
    let want = opts.limit.max(1);
    // Over-fetch so dedupe can't starve the result set below `want`.
    let fetch_opts = CodeSearchOpts {
        limit: want * 2,
        project: opts.project.clone(),
        language: opts.language.clone(),
        path: opts.path.clone(),
        group_by_file: false,
    };
    let raw = search_code(db, embedder, query, &fetch_opts).await?;

    let project = opts.project.as_deref().unwrap_or("global");
    let mut out: Vec<CodeSemanticHit> = Vec::new();

    let is_dup = |out: &[CodeSemanticHit], hit: &CodeSemanticHit| {
        out.iter().any(|k| match (&k.qualified, &hit.qualified) {
            (Some(a), Some(b)) => a == b,
            _ => {
                k.path == hit.path
                    && hit.start_line <= k.end_line.saturating_add(1)
                    && k.start_line <= hit.end_line.saturating_add(1)
            }
        })
    };

    'outer: for r in raw {
        let chunk = (r.start_line, r.end_line);
        // What does the matched region define? Fall back to its enclosing symbol.
        let mut syms = symbols_defined_in(db, project, &r.path, chunk.0, chunk.1).await;
        if syms.is_empty()
            && let Some(s) = enclosing_symbol(db, project, &r.path, chunk.0).await
        {
            syms.push(s);
        }

        if syms.is_empty() {
            // Non-symbol chunk (imports / comments / markdown / config): pointer.
            let hit = CodeSemanticHit {
                path: r.path,
                start_line: chunk.0,
                end_line: chunk.1,
                score: r.score,
                qualified: None,
                kind: None,
                signature: None,
            };
            if !is_dup(&out, &hit) {
                out.push(hit);
            }
        } else {
            for s in syms {
                let hit = s.into_hit(&r.path, r.score, chunk);
                if !is_dup(&out, &hit) {
                    out.push(hit);
                }
                if out.len() >= want {
                    break 'outer;
                }
            }
        }
        if out.len() >= want {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_hit_omits_empty_symbol_fields() {
        // Non-symbol hit serializes as a bare pointer (no qualified/kind/signature keys).
        let ptr = CodeSemanticHit {
            path: "README.md".into(),
            start_line: 1,
            end_line: 4,
            score: 0.8,
            qualified: None,
            kind: None,
            signature: None,
        };
        let json = serde_json::to_value(&ptr).unwrap();
        for absent in ["qualified", "kind", "signature", "snippet"] {
            assert!(json.get(absent).is_none(), "{absent} should be omitted");
        }
        for present in ["path", "start_line", "end_line", "score"] {
            assert!(json.get(present).is_some(), "{present} missing");
        }
    }
}
