//! LLM-as-reranker — listwise second-stage retrieval via a local LLM.
//!
//! One Ollama call per search: feed the query + top-K candidate docs, ask
//! for a JSON-encoded best-to-worst ordering, re-sort results accordingly.
//!
//! Chosen over a cross-encoder (e.g. `bge-reranker-base`) because the
//! hifz corpus is code/config text. MS-MARCO-trained rerankers score
//! poorly on that distribution — a bench in Phase 10 showed `bge-base`
//! dropping Recall@5 from 0.944 to 0.900. A generalist LLM carries the
//! world knowledge needed to map e.g. *"ORM choice"* → *"sqlx"*.
//!
//! Contract: on any failure (Ollama unreachable, malformed JSON, invalid
//! permutation) the original `results` are returned unchanged and the
//! error is logged. No hard dependency on the LLM succeeding.

use serde::Deserialize;

use crate::models::SearchResult;
use crate::ollama::OllamaClient;

const SYSTEM_PROMPT: &str = "You are a search-result reranker. \
Given a user query and a numbered list of candidate documents, return \
a JSON object with a single field \"order\" containing the 0-based \
indices of the documents, from most relevant to least. \
Every input index MUST appear exactly once. \
Respond with JSON only — no prose, no code fences.";

/// Truncate a document snippet passed to the LLM. Keeps prompt cost bounded.
const DOC_CHARS: usize = 240;

#[derive(Debug, Deserialize)]
struct RerankOrder {
    order: Vec<usize>,
}

/// Re-score the top-`top_n` memory rows in `results` using `ollama` as a
/// listwise reranker. Observations pass through unchanged. The full list
/// is re-sorted at the end so the reranked memories may move above/below
/// observations based on the new scores.
pub async fn apply_llm_rerank(
    ollama: &OllamaClient,
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

    // Two memories is the minimum where ordering is a meaningful concept.
    if mem_indices.len() < 2 {
        return results;
    }

    let user_prompt = build_prompt(query, &mem_indices, &results);
    let raw = match ollama.complete(SYSTEM_PROMPT, &user_prompt).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("llm_rerank: ollama call failed: {e}");
            return results;
        }
    };

    let parsed = match parse_order(&raw, mem_indices.len()) {
        Some(p) => p,
        None => return results, // parse_order already logged
    };

    // Map local index → new score (higher = better). Observations keep theirs.
    let n = parsed.len() as f64;
    for (new_rank, &local_idx) in parsed.iter().enumerate() {
        let r_idx = mem_indices[local_idx];
        results[r_idx].score = Some((n - new_rank as f64) / n);
    }

    results.sort_by(|a, b| {
        b.score
            .unwrap_or(0.0)
            .partial_cmp(&a.score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

fn build_prompt(query: &str, mem_indices: &[usize], results: &[SearchResult]) -> String {
    let mut s = String::with_capacity(512 + mem_indices.len() * (DOC_CHARS + 64));
    s.push_str("Query: \"");
    s.push_str(query);
    s.push_str("\"\n\nDocuments:\n");
    for (local, &r_idx) in mem_indices.iter().enumerate() {
        let r = &results[r_idx];
        let snippet = truncate(&r.narrative, DOC_CHARS);
        s.push_str(&format!("{local}. {} — {}\n", r.title, snippet));
    }
    s.push_str(&format!(
        "\nRespond with strict JSON: {{\"order\": [indices 0..{}, each exactly once, best first]}}",
        mem_indices.len() - 1
    ));
    s
}

/// Extract `{...}` from `raw`, parse, validate it's a permutation of `0..n`.
/// Returns `Some(order)` on success or `None` (after logging) on any failure.
fn parse_order(raw: &str, n: usize) -> Option<Vec<usize>> {
    let (start, end) = match (raw.find('{'), raw.rfind('}')) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => {
            tracing::warn!(
                "llm_rerank: no JSON object in output; raw: {}",
                truncate(raw, 200)
            );
            return None;
        }
    };
    let slice = &raw[start..=end];
    let parsed: RerankOrder = match serde_json::from_str(slice) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                "llm_rerank: JSON parse failed ({e}); raw: {}",
                truncate(raw, 200)
            );
            return None;
        }
    };
    if parsed.order.len() != n {
        tracing::warn!(
            "llm_rerank: expected order of length {n}, got {}",
            parsed.order.len()
        );
        return None;
    }
    let mut seen = vec![false; n];
    for &i in &parsed.order {
        if i >= n || seen[i] {
            tracing::warn!(
                "llm_rerank: invalid order (duplicate or out-of-range): {:?}",
                parsed.order
            );
            return None;
        }
        seen[i] = true;
    }
    Some(parsed.order)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json() {
        let o = parse_order(r#"{"order":[2,0,1]}"#, 3).unwrap();
        assert_eq!(o, vec![2, 0, 1]);
    }

    #[test]
    fn parses_json_wrapped_in_prose() {
        let o = parse_order(
            "Here is the ranking: {\"order\": [1, 0]} — reasoning follows…",
            2,
        )
        .unwrap();
        assert_eq!(o, vec![1, 0]);
    }

    #[test]
    fn rejects_duplicate_indices() {
        assert!(parse_order(r#"{"order":[0,0,1]}"#, 3).is_none());
    }

    #[test]
    fn rejects_out_of_range() {
        assert!(parse_order(r#"{"order":[0,1,5]}"#, 3).is_none());
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(parse_order(r#"{"order":[0,1]}"#, 3).is_none());
    }

    #[test]
    fn rejects_missing_json() {
        assert!(parse_order("no json here", 3).is_none());
    }
}
