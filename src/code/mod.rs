//! Code indexing + memory↔code cross-linking.
//!
//! Native-Rust port of cocoindex-code's chunking + indexing pipeline. Memories
//! cross-link to code at two granularities:
//! - chunk-level via the `references` edge (precise line ranges)
//! - symbol-level via the `references_symbol` edge (named functions/structs/...)
//!
//! Re-indexing is idempotent: `code_file.mtime_ns` + `content_hash` short-circuit
//! unchanged files. When a file changes, `re_anchor_references` rewrites edges
//! to point at the new chunk that overlaps the same line range — keeping the
//! "memory references a precise point in code" graph durable across edits.
//!
//! Compiled into every hifz build — no feature flag.
//!
//! ## Surface (this module set)
//! - `lang`     — extension → tree-sitter `Language` mapping
//! - `walker`   — gitignore-honest file walking with binary + size guards
//! - `splitter` — language-aware chunking via `text-splitter::CodeSplitter`,
//!   fallback to `crate::chunk::split` for unsupported types
//! - `symbols`  — function/struct/enum/... extraction via tree-sitter `Query`
//!
//! ## Also provides
//! - `index`, `search`, `link`, `gc`, `watcher`

pub mod codeintel;
pub mod gc;
pub mod index;
pub mod link;
pub mod search;
pub mod splitter;
pub mod watcher;

// `lang` + `walker` moved to `kernel::code_parse`; re-exported so
// `crate::code::lang::*` / `crate::code::walker::*` resolve unchanged.
pub use kernel::code_parse::{lang, walker};
