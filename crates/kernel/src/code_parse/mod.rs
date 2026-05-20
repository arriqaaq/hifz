//! Code parsing primitives shared by hifz indexing and maktab.
//!
//! `lang` (extension → tree-sitter grammar) and `walker` (gitignore-respecting
//! file discovery), plus the code-intelligence core (`langcfg`, `langmod`,
//! `codegraph`, `coderesolve`).

pub mod lang;
pub mod walker;

// Code-intelligence core: one imperative walk → semantic
// scope-qualified identity → scope/import resolution. No `.scm`.
pub mod codegraph;
pub mod coderesolve;
pub mod langcfg;
pub mod langmod;
