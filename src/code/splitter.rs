//! Language-aware code chunking.
//!
//! Wraps `text_splitter::CodeSplitter` for languages with a registered
//! tree-sitter grammar; falls back to `crate::chunk::split` for `Plain`.
//!
//! Defaults mirror cocoindex-code: target ~1000 chars, overlap 150, min 250.
//! The byte-offset → 1-indexed line mapping is computed once per file and
//! reused for every chunk via binary search on a `line_starts` vector.

use anyhow::Result;
use text_splitter::{ChunkConfig, CodeSplitter as TsCodeSplitter};

use crate::code::lang::Language;

const DEFAULT_TARGET_CHARS: usize = 1000;
const DEFAULT_OVERLAP_CHARS: usize = 150;
const DEFAULT_MIN_CHARS: usize = 250;

#[derive(Debug, Clone)]
pub struct RawChunk {
    pub content: String,
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1-indexed inclusive
    pub start_line: usize,
    /// 1-indexed inclusive
    pub end_line: usize,
}

#[derive(Debug, Clone)]
pub struct CodeSplitter {
    target_chars: usize,
    overlap_chars: usize,
    min_chars: usize,
}

impl Default for CodeSplitter {
    fn default() -> Self {
        Self {
            target_chars: DEFAULT_TARGET_CHARS,
            overlap_chars: DEFAULT_OVERLAP_CHARS,
            min_chars: DEFAULT_MIN_CHARS,
        }
    }
}

impl CodeSplitter {
    pub fn new(target_chars: usize, overlap_chars: usize, min_chars: usize) -> Self {
        Self {
            target_chars,
            overlap_chars,
            min_chars,
        }
    }

    /// Per-language defaults. M1 uses uniform values; M6 may tune by language.
    pub fn for_lang(_lang: Language) -> Self {
        Self::default()
    }

    pub fn split(&self, lang: Language, source: &str) -> Result<Vec<RawChunk>> {
        if source.trim().is_empty() {
            return Ok(Vec::new());
        }
        let line_starts = compute_line_starts(source);

        let mut out: Vec<RawChunk> = Vec::new();

        if let Some(ts_lang) = lang.ts_language() {
            let cfg = ChunkConfig::new(self.target_chars).with_overlap(self.overlap_chars)?;
            let splitter = TsCodeSplitter::new(ts_lang, cfg)?;
            for (start, chunk) in splitter.chunk_indices(source) {
                let end = start + chunk.len();
                if chunk.len() < self.min_chars && !out.is_empty() {
                    continue;
                }
                let start_line = byte_to_line(start, &line_starts);
                let end_line = byte_to_line(end.saturating_sub(1).max(start), &line_starts);
                out.push(RawChunk {
                    content: chunk.to_string(),
                    start_byte: start,
                    end_byte: end,
                    start_line,
                    end_line,
                });
            }
        } else {
            // Plain fallback via the existing markdown-aware text splitter.
            // `chunk::split` doesn't carry byte offsets, so we recover them
            // by searching the source. The fallback is rare (only Plain) so
            // O(n*m) here is acceptable.
            let mut search_from = 0usize;
            for c in crate::chunk::split(source) {
                let needle = c.content.as_str();
                let start = match source[search_from..].find(needle) {
                    Some(p) => search_from + p,
                    None => match source.find(needle) {
                        Some(p) => p,
                        None => continue,
                    },
                };
                let end = start + needle.len();
                search_from = end;
                let start_line = byte_to_line(start, &line_starts);
                let end_line = byte_to_line(end.saturating_sub(1).max(start), &line_starts);
                out.push(RawChunk {
                    content: c.content,
                    start_byte: start,
                    end_byte: end,
                    start_line,
                    end_line,
                });
            }
        }

        Ok(out)
    }
}

fn compute_line_starts(s: &str) -> Vec<usize> {
    let mut v = Vec::with_capacity(64);
    v.push(0);
    for (i, b) in s.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

/// Map a byte offset to a 1-indexed line number.
fn byte_to_line(byte: usize, line_starts: &[usize]) -> usize {
    match line_starts.binary_search(&byte) {
        Ok(idx) => idx + 1,
        Err(idx) => idx.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_yields_no_chunks() {
        let s = CodeSplitter::default();
        assert!(s.split(Language::Rust, "").unwrap().is_empty());
        assert!(s.split(Language::Rust, "\n\n  \t").unwrap().is_empty());
    }

    #[test]
    fn rust_short_file_is_one_chunk_with_line_one() {
        let s = CodeSplitter::default();
        let src = "fn main() {\n    println!(\"hi\");\n}\n";
        let chunks = s.split(Language::Rust, src).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_line, 1);
        assert!(chunks[0].end_line >= 3);
        assert_eq!(chunks[0].start_byte, 0);
        assert_eq!(chunks[0].end_byte, src.len());
    }

    #[test]
    fn line_numbers_are_one_indexed() {
        let s = CodeSplitter::new(80, 20, 30); // small chunks to force splits
        let mut src = String::new();
        for i in 0..20 {
            src.push_str(&format!("fn f{i}() {{ let x = {i}; }}\n"));
        }
        let chunks = s.split(Language::Rust, &src).unwrap();
        assert!(
            chunks.len() >= 2,
            "expected ≥2 chunks, got {}",
            chunks.len()
        );
        // First chunk starts at line 1.
        assert_eq!(chunks[0].start_line, 1);
        // Lines are monotonically non-decreasing across chunks.
        for w in chunks.windows(2) {
            assert!(
                w[1].start_line >= w[0].start_line,
                "lines went backwards: {} → {}",
                w[0].start_line,
                w[1].start_line
            );
        }
    }

    #[test]
    fn python_chunks_have_valid_line_ranges() {
        let s = CodeSplitter::default();
        let src = "def a():\n    return 1\n\ndef b():\n    return 2\n";
        let chunks = s.split(Language::Python, src).unwrap();
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(c.start_line >= 1);
            assert!(c.end_line >= c.start_line);
        }
    }

    #[test]
    fn plain_fallback_works_for_unsupported() {
        // chunk::split fallback is exercised when ts_language returns None.
        let s = CodeSplitter::default();
        let src = "This is some plain text. ".repeat(200);
        let chunks = s.split(Language::Plain, &src).unwrap();
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].start_line, 1);
    }

    #[test]
    fn byte_to_line_is_one_indexed() {
        let starts = compute_line_starts("a\nbb\nccc\n"); // [0, 2, 5, 9]
        assert_eq!(byte_to_line(0, &starts), 1);
        assert_eq!(byte_to_line(1, &starts), 1);
        assert_eq!(byte_to_line(2, &starts), 2);
        assert_eq!(byte_to_line(4, &starts), 2);
        assert_eq!(byte_to_line(5, &starts), 3);
    }
}
