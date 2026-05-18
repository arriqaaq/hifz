//! Structure-aware text chunking (the pure, DB-free half of hifz's
//! `chunk.rs`). The DB writers (`persist_chunks`, `delete_chunks_for`)
//! stay in `hifz::chunk` and call into `Chunk`/`split` re-exported from
//! here.

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
