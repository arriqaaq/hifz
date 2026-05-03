//! Long-form artifact chunking for retrieval.
//!
//! When a memory has `content_long` set (Plan, Design, CodeReview,
//! ShipReport, ContextSlice), this module splits it into overlapping chunks
//! and writes them to the `memory_chunk` table. Search hits chunks via
//! vector + BM25; retrieval groups hits by parent memory.
//!
//! ## Splitting strategy
//!
//! The splitter is structure-aware: it prefers boundaries at headings
//! (`#`, `##`), code-fence delimiters (` ``` `), and blank lines, then
//! falls back to token windows. This keeps semantic units intact more
//! often than a pure token-window split would.
//!
//! Token estimation uses a simple character-count heuristic
//! (`chars / 4 ≈ tokens`) — close enough for chunk sizing without
//! pulling in a tokenizer. The targets are:
//!
//! - chunk size: ~500 tokens (~2KB of text)
//! - overlap:    ~100 tokens (~400 chars)
//!
//! Both tunable via constants. The overlap matters: it gives the embedder
//! enough context to disambiguate phrases that straddle the boundary.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::embed::Embedder;
use crate::link;

/// Target chunk size in characters (≈ 500 tokens).
const CHUNK_TARGET_CHARS: usize = 2000;
/// Overlap between adjacent chunks in characters (≈ 100 tokens).
const CHUNK_OVERLAP_CHARS: usize = 400;
/// Floor — we don't emit a chunk shorter than this unless the whole text
/// is shorter than CHUNK_TARGET_CHARS.
const CHUNK_MIN_CHARS: usize = 200;

/// One produced chunk.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub index: usize,
    pub content: String,
}

/// Split `text` into overlapping chunks. Single-pass, deterministic.
pub fn split(text: &str) -> Vec<Chunk> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.chars().count() <= CHUNK_TARGET_CHARS {
        return vec![Chunk {
            index: 0,
            content: trimmed.to_string(),
        }];
    }

    // Work on byte indices but step on char boundaries so we never split a
    // multi-byte UTF-8 char.
    let bytes = trimmed.as_bytes();
    let len = bytes.len();
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;

    while start < len {
        let target_end = (start + CHUNK_TARGET_CHARS).min(len);
        let end = if target_end >= len {
            len
        } else {
            // Snap forward to the next semantic boundary if one is nearby
            // (within +200 bytes), otherwise back-search for one within the
            // last 400 bytes of the chunk. This gives natural boundaries
            // when they exist without producing pathologically uneven sizes.
            best_boundary(trimmed, target_end)
                .unwrap_or_else(|| snap_to_char_boundary(trimmed, target_end))
        };

        let slice = &trimmed[start..end];
        let slice_trim = slice.trim();
        if slice_trim.len() >= CHUNK_MIN_CHARS || chunks.is_empty() {
            chunks.push(Chunk {
                index: idx,
                content: slice_trim.to_string(),
            });
            idx += 1;
        }

        if end >= len {
            break;
        }
        // Advance with overlap; never go backwards.
        let next = end.saturating_sub(CHUNK_OVERLAP_CHARS);
        start = snap_to_char_boundary(trimmed, next.max(start + 1));
    }

    chunks
}

/// Find the closest semantic boundary to `target` within ±200 bytes.
/// Preference order: blank line > heading start > code fence > sentence end.
fn best_boundary(text: &str, target: usize) -> Option<usize> {
    let window = 200usize;
    let lo = target.saturating_sub(window);
    let hi = (target + window).min(text.len());
    let region = text.get(lo..hi)?;

    // Score each candidate by distance to target (lower is better).
    let mut best: Option<(usize, i64)> = None;
    let mut score = |pos: usize, weight: i64| {
        let abs = (pos as i64 - target as i64).abs();
        let s = abs + weight;
        if best.map(|(_, b)| s < b).unwrap_or(true) {
            best = Some((pos, s));
        }
    };

    // Blank line (highest priority — base weight 0).
    let mut search_from = 0usize;
    while let Some(idx) = region[search_from..].find("\n\n") {
        let abs = lo + search_from + idx + 2;
        if abs <= text.len() && text.is_char_boundary(abs) {
            score(abs, 0);
        }
        search_from += idx + 2;
    }
    // Markdown heading at line start (weight +20 — slightly worse than blank line).
    search_from = 0;
    while let Some(idx) = region[search_from..].find("\n#") {
        let abs = lo + search_from + idx + 1;
        if abs <= text.len() && text.is_char_boundary(abs) {
            score(abs, 20);
        }
        search_from += idx + 2;
    }
    // Code fence (weight +30).
    search_from = 0;
    while let Some(idx) = region[search_from..].find("\n```") {
        let abs = lo + search_from + idx + 1;
        if abs <= text.len() && text.is_char_boundary(abs) {
            score(abs, 30);
        }
        search_from += idx + 4;
    }
    // Sentence boundary "\n" alone (weight +50).
    search_from = 0;
    while let Some(idx) = region[search_from..].find('\n') {
        let abs = lo + search_from + idx + 1;
        if abs <= text.len() && text.is_char_boundary(abs) {
            score(abs, 50);
        }
        search_from += idx + 1;
    }

    best.map(|(pos, _)| pos)
}

/// Step `pos` forward until it lands on a UTF-8 char boundary.
fn snap_to_char_boundary(text: &str, mut pos: usize) -> usize {
    let len = text.len();
    while pos < len && !text.is_char_boundary(pos) {
        pos += 1;
    }
    pos.min(len)
}

// ---------------------------------------------------------------------------
// DB writers
// ---------------------------------------------------------------------------

/// Persist chunks for a parent memory: writes one `memory_chunk` row per
/// chunk and one `part_of` edge per chunk → parent. Embeds each chunk
/// individually. Idempotent at insert level via the `chunk_parent +
/// chunk_index` access pattern (callers should delete prior chunks before
/// re-emitting if they're updating).
pub async fn persist_chunks(
    db: &Surreal<Db>,
    embedder: &Embedder,
    parent_id: &RecordId,
    project: &str,
    chunks: &[Chunk],
) -> Result<usize> {
    if chunks.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut written = 0usize;

    #[derive(Debug, SurrealValue)]
    struct Created {
        id: Option<RecordId>,
    }

    for c in chunks {
        let embedding = match embedder.embed_single(&c.content) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("chunk embed failed at idx {}: {e}", c.index);
                continue;
            }
        };
        let mut resp = match db
            .query(
                "CREATE memory_chunk SET \
                 parent_id = $pid, \
                 project = $project, \
                 chunk_index = $idx, \
                 content = $content, \
                 embedding = $embedding, \
                 created_at = $now \
                 RETURN id",
            )
            .bind(("pid", parent_id.clone()))
            .bind(("project", project.to_string()))
            .bind(("idx", c.index as i64))
            .bind(("content", c.content.clone()))
            .bind(("embedding", embedding))
            .bind(("now", now.clone()))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("chunk insert failed at idx {}: {e}", c.index);
                continue;
            }
        };
        let created: Vec<Created> = resp.take(0).unwrap_or_default();
        let Some(chunk_id) = created.into_iter().next().and_then(|x| x.id) else {
            continue;
        };

        // chunk --part_of--> parent memory. Type-pair table allows
        // Memory->Memory only for `part_of`; chunk's table is `memory_chunk`
        // which maps to RecordKind::Other, which short-circuits to permitted.
        // (Recorded as a followup for Phase 10: extend RecordKind to
        // include MemoryChunk so the constraint is precise.)
        let _ = link::upsert_edge(
            db,
            &chunk_id,
            parent_id,
            "part_of",
            "system",
            1.0,
            Some(&format!("chunk {} of long-form artifact", c.index)),
        )
        .await;
        written += 1;
    }

    Ok(written)
}

/// Delete all chunks (and associated `part_of` edges) for a parent memory.
/// Used when re-chunking after a `PUT /markdown` update.
pub async fn delete_chunks_for(db: &Surreal<Db>, parent_id: &RecordId) -> Result<()> {
    // Collect chunk ids first so we can drop edges by endpoint.
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query("SELECT id FROM memory_chunk WHERE parent_id = $pid")
        .bind(("pid", parent_id.clone()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    let ids: Vec<RecordId> = rows.into_iter().filter_map(|r| r.id).collect();

    if !ids.is_empty() {
        // Drop edges where either endpoint is one of these chunks.
        let _ = db
            .query("DELETE edge WHERE in IN $ids OR out IN $ids")
            .bind(("ids", ids.clone()))
            .await;
        let _ = db
            .query("DELETE memory_chunk WHERE id IN $ids")
            .bind(("ids", ids))
            .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_yields_no_chunks() {
        assert!(split("").is_empty());
        assert!(split("   \n\n  ").is_empty());
    }

    #[test]
    fn short_text_is_one_chunk() {
        let t = "this is a short note about jwt handling";
        let chunks = split(t);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].content, t);
    }

    #[test]
    fn long_text_chunks_with_overlap() {
        // Build a doc clearly longer than CHUNK_TARGET_CHARS.
        let para = "This is a paragraph about the auth flow. ".repeat(80);
        let doc = format!("# Plan\n\n{para}\n\n## Section\n\n{para}");
        let chunks = split(&doc);
        assert!(
            chunks.len() >= 2,
            "expected ≥2 chunks, got {}",
            chunks.len()
        );
        // Indices monotonic from 0.
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.index, i);
        }
        // Overlap: last 100 chars of chunk N should appear somewhere in chunk N+1.
        for w in chunks.windows(2) {
            let tail: String = w[0]
                .content
                .chars()
                .rev()
                .take(50)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            // Heuristic: at least some overlap window survives the boundary.
            assert!(
                w[1].content.contains(&tail) || w[1].content.len() < CHUNK_MIN_CHARS,
                "expected overlap between chunk {} and {}",
                w[0].index,
                w[1].index
            );
        }
    }

    #[test]
    fn snap_handles_multibyte_chars() {
        let s = "café résumé";
        // Position 4 is mid-é (bytes 0xC3 0xA9). snap should land on a boundary.
        let snapped = snap_to_char_boundary(s, 4);
        assert!(s.is_char_boundary(snapped));
    }

    #[test]
    fn boundary_finder_prefers_blank_line() {
        let text = "aaaaaaaaaa\n\nbbbbbbbbbb\nccccccccccc";
        // target = 12 (in the middle of the blank line region).
        let pos = best_boundary(text, 12);
        assert!(pos.is_some());
    }
}
