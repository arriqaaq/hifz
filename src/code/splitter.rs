//! Language-aware code chunking.
//!
//! Wraps `text_splitter::CodeSplitter` for languages with a registered
//! tree-sitter grammar; falls back to `crate::chunk::split` for `Plain`.
//!
//! Target ~1000 chars, overlap 150 (inherited from cocoindex-code). The
//! sub-min-size chunk **drop was removed** (the `min_chars` knob is gone): a
//! faithfulness-gated 6-cell ablation on the hifz corpus showed it
//! *consistently lost* retrieval (−2.7…−3.6pp chunk-hit@10 across all
//! overlaps, corroborated by strict-span coverage loss) for only ~4% byte
//! saving. Overlap is left at 150 pending a decision — the same ablation
//! found it has no measured retrieval effect (0/150/300 flat) but it improves
//! whole-symbol coverage, which the retrieval metric cannot score; changing
//! it awaits a consuming-agent metric. Evidence: docs/eval/code-retrieval.md.
//! The byte-offset → 1-indexed line mapping is computed once per file and
//! reused for every chunk via binary search on a `line_starts` vector.

use anyhow::Result;
use text_splitter::{ChunkConfig, CodeSplitter as TsCodeSplitter};

use crate::code::lang::Language;

const DEFAULT_TARGET_CHARS: usize = 1000;
const DEFAULT_OVERLAP_CHARS: usize = 150;

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
}

impl Default for CodeSplitter {
    fn default() -> Self {
        Self {
            target_chars: DEFAULT_TARGET_CHARS,
            overlap_chars: DEFAULT_OVERLAP_CHARS,
        }
    }
}

impl CodeSplitter {
    pub fn new(target_chars: usize, overlap_chars: usize) -> Self {
        Self {
            target_chars,
            overlap_chars,
        }
    }

    /// Per-language defaults. Uses uniform values; may tune per language later.
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

        // `text_splitter` trims trailing whitespace off each chunk, so the
        // final chunk's `end_byte` can stop short of EOF (e.g. a file
        // ending in `\n`). When the only thing between the last chunk and
        // EOF is whitespace, extend it so chunks tile the whole file.
        if let Some(last) = out.last_mut()
            && last.end_byte < source.len()
            && source[last.end_byte..].chars().all(char::is_whitespace)
        {
            last.end_byte = source.len();
            last.end_line = byte_to_line(
                source.len().saturating_sub(1).max(last.start_byte),
                &line_starts,
            );
        }

        let out = coalesce_structural(out, source, &line_starts);
        Ok(coalesce_preamble(out, source, &line_starts))
    }
}

/// True if the chunk carries any retrieval signal (an identifier/word char).
/// A lone `{`, `}`, `=> {`, `()` etc. has none.
fn has_word(s: &str) -> bool {
    s.chars().any(|c| c.is_alphanumeric() || c == '_')
}

/// Re-slice a chunk's content from `source` by its (possibly merged) byte
/// range and recompute its 1-indexed line numbers. Used after merging so
/// content and line numbers stay exactly consistent with the merged span.
fn reslice(c: &mut RawChunk, source: &str, line_starts: &[usize]) {
    c.content = source[c.start_byte..c.end_byte].to_string();
    c.start_line = byte_to_line(c.start_byte, line_starts);
    c.end_line = byte_to_line(c.end_byte.saturating_sub(1).max(c.start_byte), line_starts);
}

/// True if the chunk is a "preamble": every non-blank line is a line comment
/// (`//`, `///`, `//!`) or an attribute (`#[…]`, `#![…]`) — i.e. it carries no
/// actual code. `text_splitter` emits these when a function's leading
/// doc-comments + attributes don't fit in the same chunk as the (oversized but
/// whole) `fn` item that follows, orphaning the doc/`#[test]` from its body.
/// Such a chunk advertises a symbol it doesn't contain. Conservative by design
/// (Rust line-comment/attr prefixes only) so it never misclassifies real code
/// such as a leading deref (`*x = …`) or a block-comment continuation line.
fn is_preamble(s: &str) -> bool {
    let mut saw_any = false;
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        saw_any = true;
        let is_line_comment = t.starts_with("//");
        let is_attr = t.starts_with("#[") || t.starts_with("#![");
        if !(is_line_comment || is_attr) {
            return false;
        }
    }
    saw_any
}

/// Fold preamble chunks (doc-comments + attributes, no code — see
/// [`is_preamble`]) FORWARD into the next chunk, the item they document.
/// Mirrors [`coalesce_structural`] but in the opposite direction: a `{` belongs
/// to the code before it, whereas a doc/`#[test]` belongs to the code after it.
/// Consecutive preambles accumulate onto the first following code chunk. A
/// trailing preamble (EOF, nothing to attach to) is kept as-is. Nothing is
/// dropped; merged ranges only grow, so `chunk_span`/symbol/memory links stay
/// resolvable.
fn coalesce_preamble(chunks: Vec<RawChunk>, source: &str, line_starts: &[usize]) -> Vec<RawChunk> {
    if chunks.len() < 2 {
        return chunks;
    }
    let mut out: Vec<RawChunk> = Vec::with_capacity(chunks.len());
    let mut carry: Option<RawChunk> = None;
    for mut ch in chunks {
        if let Some(pre) = carry.take() {
            ch.start_byte = pre.start_byte.min(ch.start_byte);
            ch.end_byte = pre.end_byte.max(ch.end_byte);
            reslice(&mut ch, source, line_starts);
        }
        if is_preamble(&ch.content) {
            carry = Some(ch);
        } else {
            out.push(ch);
        }
    }
    // Trailing preamble: nothing follows to attach it to — keep it.
    if let Some(pre) = carry {
        out.push(pre);
    }
    out
}

/// Merge structural-only fragments (no word char) into a neighbor so they
/// never become standalone chunks. `text_splitter` emits these (e.g. a lone
/// `{`) when a block's opening delimiter is its own tree-sitter node and the
/// following sibling exceeds the chunk capacity; with no min-chunk knob and
/// the `min_chars` drop intentionally removed, they otherwise pollute search
/// (a `{` embeds ~equidistant from every query). We MERGE rather than drop
/// (an ablation showed dropping small chunks hurts recall), re-slicing content
/// from `source` by byte range (overlap-safe — never string-concat) and
/// recomputing line numbers from the merged span.
fn coalesce_structural(
    chunks: Vec<RawChunk>,
    source: &str,
    line_starts: &[usize],
) -> Vec<RawChunk> {
    if chunks.len() < 2 {
        return chunks;
    }
    let mut out: Vec<RawChunk> = Vec::with_capacity(chunks.len());
    for ch in chunks {
        if !has_word(&ch.content) {
            if let Some(prev) = out.last_mut() {
                prev.start_byte = ch.start_byte.min(prev.start_byte);
                prev.end_byte = ch.end_byte.max(prev.end_byte);
                reslice(prev, source, line_starts);
                continue;
            }
        }
        out.push(ch);
    }
    // Leading structural fragment (nothing before it) → fold into the next.
    if out.len() > 1 && !has_word(&out[0].content) {
        let first = out.remove(0);
        let nxt = &mut out[0];
        nxt.start_byte = first.start_byte.min(nxt.start_byte);
        nxt.end_byte = first.end_byte.max(nxt.end_byte);
        reslice(nxt, source, line_starts);
    }
    out
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
        let s = CodeSplitter::new(80, 20); // small chunks to force splits
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

    /// Build a RawChunk for `source[a..b]` with correct line numbers.
    fn rc(source: &str, ls: &[usize], a: usize, b: usize) -> RawChunk {
        RawChunk {
            content: source[a..b].to_string(),
            start_byte: a,
            end_byte: b,
            start_line: byte_to_line(a, ls),
            end_line: byte_to_line(b.saturating_sub(1).max(a), ls),
        }
    }

    #[test]
    fn coalesce_merges_lone_brace() {
        let source = "let a = 1;\n{\nlet b = 2;\n";
        let ls = compute_line_starts(source);
        // "let a = 1;" (0..10), "{" (11..12), "let b = 2;" (13..23)
        let chunks = vec![
            rc(source, &ls, 0, 10),
            rc(source, &ls, 11, 12),
            rc(source, &ls, 13, 23),
        ];
        let out = coalesce_structural(chunks, source, &ls);
        assert!(
            out.iter().all(|c| has_word(&c.content)),
            "no structural-only chunks should remain: {out:?}"
        );
        assert_eq!(out.len(), 2);
        assert!(
            out[0].content.contains('{'),
            "the brace was absorbed into a neighbor"
        );
    }

    #[test]
    fn coalesce_keeps_small_word_chunks() {
        let source = "let a = 1;\nmatch self\n";
        let ls = compute_line_starts(source);
        // "let a = 1;" (0..10), "match self" (11..21)
        let chunks = vec![rc(source, &ls, 0, 10), rc(source, &ls, 11, 21)];
        let out = coalesce_structural(chunks, source, &ls);
        assert_eq!(out.len(), 2, "a small but word-bearing chunk is kept");
        assert_eq!(out[1].content, "match self");
    }

    #[test]
    fn coalesce_handles_leading_fragment() {
        let source = "{\nlet b = 2;\n";
        let ls = compute_line_starts(source);
        // "{" (0..1), "let b = 2;" (2..12)
        let chunks = vec![rc(source, &ls, 0, 1), rc(source, &ls, 2, 12)];
        let out = coalesce_structural(chunks, source, &ls);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].content.starts_with('{'),
            "leading brace folded into next"
        );
        assert_eq!(out[0].start_line, 1);
    }

    /// End-to-end through the real `text_splitter`: a function whose body is a
    /// single oversized statement forces text_splitter to isolate the opening
    /// `{` (the production bug). After coalescing, NO chunk may be
    /// structural-only. (Removing the coalesce makes this fail.)
    #[test]
    fn split_never_emits_structural_only_chunks() {
        let big = "x".repeat(2000);
        let src = format!(
            "fn huge() {{\n    let s = \"{big}\";\n}}\n\nfn small() {{\n    let y = 1;\n}}\n"
        );
        let chunks = CodeSplitter::default().split(Language::Rust, &src).unwrap();
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(
                has_word(&c.content),
                "structural-only chunk escaped split(): {:?} @ L{}-{}",
                c.content,
                c.start_line,
                c.end_line
            );
            // line range stays consistent with content
            assert!(c.end_line >= c.start_line);
        }
    }

    /// Sanity: the same oversized input, but assert text_splitter really does
    /// produce the offending lone `{` *before* coalescing — so the test above
    /// is exercising the real failure, not a vacuous pass.
    #[test]
    fn raw_text_splitter_isolates_lone_brace() {
        let big = "x".repeat(2000);
        let src = format!("fn huge() {{\n    let s = \"{big}\";\n}}\n");
        let line_starts = compute_line_starts(&src);
        // Reproduce the raw (pre-coalesce) split: run text_splitter directly.
        let cfg = ChunkConfig::new(DEFAULT_TARGET_CHARS)
            .with_overlap(DEFAULT_OVERLAP_CHARS)
            .unwrap();
        let ts = TsCodeSplitter::new(Language::Rust.ts_language().unwrap(), cfg).unwrap();
        let raw: Vec<String> = ts.chunk_indices(&src).map(|(_, c)| c.to_string()).collect();
        let _ = &line_starts;
        assert!(
            raw.iter().any(|c| !has_word(c)),
            "expected text_splitter to emit a structural-only chunk for an oversized body; got {raw:?}"
        );
    }

    #[test]
    fn is_preamble_detects_doc_and_attr() {
        assert!(is_preamble("/// doc one\n/// doc two\n#[test]"));
        assert!(is_preamble("//! module doc\n"));
        assert!(is_preamble(
            "// plain comment\n#[derive(Debug)]\n#![allow(dead_code)]"
        ));
        assert!(is_preamble("\n  /// indented doc \n\n#[inline]\n"));
        // Empty / whitespace-only is not a preamble (nothing to attach).
        assert!(!is_preamble("   \n\n"));
    }

    #[test]
    fn is_preamble_rejects_code() {
        // A real `fn`/statement makes it not a preamble.
        assert!(!is_preamble("/// doc\n#[test]\nfn f() {}"));
        // A leading deref must NOT be mistaken for a comment-continuation `*`.
        assert!(!is_preamble("*self.count += 1;\n*other -= 1;"));
        assert!(!is_preamble("let x = 1;"));
    }

    #[test]
    fn coalesce_preamble_folds_doc_attr_forward() {
        // The exact dst shape: a doc-comment + `#[test]` orphaned from its fn.
        let source =
            "/// docs about the test below\n#[test]\nfn release_after_due() {\n    body();\n}\n";
        let ls = compute_line_starts(source);
        let split = source.find("fn ").unwrap(); // boundary between preamble and fn
        let chunks = vec![
            rc(source, &ls, 0, split),
            rc(source, &ls, split, source.len()),
        ];
        assert!(
            is_preamble(&chunks[0].content),
            "fixture chunk0 must be a preamble"
        );
        let out = coalesce_preamble(chunks, source, &ls);
        assert_eq!(out.len(), 1, "preamble folded into the fn chunk");
        assert!(out[0].content.contains("#[test]"), "doc/attr preserved");
        assert!(
            out[0].content.contains("fn release_after_due"),
            "now carries the fn it documents"
        );
        assert_eq!(
            out[0].start_line, 1,
            "range extends back over the doc comment"
        );
        assert!(!is_preamble(&out[0].content));
    }

    #[test]
    fn coalesce_preamble_accumulates_consecutive() {
        let source = "/// a\n#[test]\n// trailing note\nfn f() {\n    g();\n}\n";
        let ls = compute_line_starts(source);
        let b1 = source.find("// trailing").unwrap();
        let b2 = source.find("fn ").unwrap();
        let chunks = vec![
            rc(source, &ls, 0, b1),
            rc(source, &ls, b1, b2),
            rc(source, &ls, b2, source.len()),
        ];
        let out = coalesce_preamble(chunks, source, &ls);
        assert_eq!(out.len(), 1, "both preamble fragments fold onto the fn");
        assert!(out[0].content.contains("fn f"));
        assert_eq!(out[0].start_line, 1);
    }

    #[test]
    fn coalesce_preamble_keeps_trailing() {
        // A preamble at EOF has nothing to attach to → kept as-is, not dropped.
        let source = "fn f() {\n    g();\n}\n// trailing footer comment\n";
        let ls = compute_line_starts(source);
        let b = source.find("// trailing").unwrap();
        let chunks = vec![rc(source, &ls, 0, b), rc(source, &ls, b, source.len())];
        let out = coalesce_preamble(chunks, source, &ls);
        assert_eq!(out.len(), 2, "trailing preamble is preserved");
        assert!(is_preamble(&out[1].content));
    }

    /// End-to-end through the real `text_splitter`: doc-comments + `#[test]`
    /// preceding a function whose whole item is large enough that it can't
    /// share a chunk with the preamble forces text_splitter to orphan the
    /// preamble (the production artifact seen in dst's sim_integration.rs).
    /// After coalescing, no non-final chunk may be a preamble.
    #[test]
    fn split_does_not_orphan_doc_attr_preamble() {
        let doc: String = (0..6)
            .map(|i| {
                format!("/// documentation comment line number {i} describing behaviour here\n")
            })
            .collect();
        let body: String = (0..32)
            .map(|i| format!("    let _v{i} = compute({i});\n"))
            .collect();
        let src = format!("{doc}#[test]\nfn target() {{\n{body}}}\n");
        let chunks = CodeSplitter::default().split(Language::Rust, &src).unwrap();
        assert!(!chunks.is_empty());
        for (i, c) in chunks.iter().enumerate() {
            let is_last = i + 1 == chunks.len();
            assert!(
                is_last || !is_preamble(&c.content),
                "doc/attr preamble orphaned at L{}-{}: {:?}",
                c.start_line,
                c.end_line,
                c.content
            );
        }
        // The doc/attr no longer stands alone: it was folded onto the fn it
        // documents, so some chunk carries both the prose and the signature.
        assert!(
            chunks
                .iter()
                .any(|c| c.content.contains("documentation comment")
                    && c.content.contains("fn target")),
            "preamble was not folded onto its fn"
        );
    }

    /// Sanity: prove `text_splitter` really orphans the preamble *before*
    /// coalescing, so the test above exercises the real failure (not a vacuous
    /// pass). If this fails, the synthetic sizing no longer triggers isolation.
    #[test]
    fn raw_text_splitter_isolates_doc_attr_preamble() {
        let doc: String = (0..6)
            .map(|i| {
                format!("/// documentation comment line number {i} describing behaviour here\n")
            })
            .collect();
        let body: String = (0..32)
            .map(|i| format!("    let _v{i} = compute({i});\n"))
            .collect();
        let src = format!("{doc}#[test]\nfn target() {{\n{body}}}\n");
        let cfg = ChunkConfig::new(DEFAULT_TARGET_CHARS)
            .with_overlap(DEFAULT_OVERLAP_CHARS)
            .unwrap();
        let ts = TsCodeSplitter::new(Language::Rust.ts_language().unwrap(), cfg).unwrap();
        let raw: Vec<String> = ts.chunk_indices(&src).map(|(_, c)| c.to_string()).collect();
        assert!(
            raw.iter().any(|c| is_preamble(c)),
            "expected text_splitter to orphan a doc/attr preamble; got {raw:?}"
        );
    }

    fn collect_rs(dir: std::path::PathBuf, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name()
                    .is_some_and(|n| n == "target" || n == ".git" || n == "node_modules")
                {
                    continue;
                }
                collect_rs(p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// Real-corpus invariant: run the splitter over EVERY `.rs` file in this
    /// project and assert no chunk is structural-only and every chunk's range
    /// is well-formed. This is the production failure mode reproduced against
    /// real code, not a synthetic input.
    #[test]
    fn real_rust_sources_yield_no_structural_only_chunks() {
        let root = env!("CARGO_MANIFEST_DIR");
        let mut files = Vec::new();
        for dir in ["src", "crates", "tests", "benchmark"] {
            collect_rs(std::path::Path::new(root).join(dir), &mut files);
        }
        assert!(
            files.len() > 20,
            "expected many .rs files, found {}",
            files.len()
        );

        let splitter = CodeSplitter::default();
        let mut offenders: Vec<String> = Vec::new();
        let mut total_chunks = 0usize;
        for f in &files {
            let Ok(src) = std::fs::read_to_string(f) else {
                continue;
            };
            if src.trim().is_empty() {
                continue;
            }
            let chunks = splitter.split(Language::Rust, &src).unwrap();
            let n = chunks.len();
            for (i, c) in chunks.iter().enumerate() {
                total_chunks += 1;
                // Range well-formedness (valid, on char boundaries).
                assert!(
                    c.end_byte >= c.start_byte,
                    "{}: bad byte range",
                    f.display()
                );
                assert!(
                    c.end_line >= c.start_line,
                    "{}: bad line range",
                    f.display()
                );
                assert!(
                    src.get(c.start_byte..c.end_byte).is_some(),
                    "{}: byte range off a char boundary",
                    f.display()
                );
                // No structural-only chunk, and no non-final doc/attr preamble
                // orphaned from the item it documents.
                let bad = !has_word(&c.content) || (i + 1 != n && is_preamble(&c.content));
                if bad {
                    offenders.push(format!(
                        "{}:{}-{} {:?}",
                        f.display(),
                        c.start_line,
                        c.end_line,
                        c.content
                    ));
                }
            }
        }
        assert!(total_chunks > 0);
        assert!(
            offenders.is_empty(),
            "{} structural-only chunk(s) escaped on real .rs files:\n{}",
            offenders.len(),
            offenders.join("\n")
        );
    }
}
