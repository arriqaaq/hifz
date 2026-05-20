//! Code parsing primitives shared by hifz indexing and maktab.
//!
//! `lang` (extension → tree-sitter grammar) and `walker` (gitignore-respecting
//! file discovery), plus the code-intelligence core (`langcfg`, `langmod`,
//! `graph`, `resolve`).

pub mod lang;
pub mod walker;

// Code-intelligence core: one imperative walk → semantic
// scope-qualified identity → scope/import resolution. No `.scm`.
pub mod graph;
pub mod langcfg;
pub mod langmod;
pub mod resolve;
