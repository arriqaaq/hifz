//! Code parsing primitives shared by hifz indexing and atlas.
//!
//! M1 surface (extracted from hifz `src/code/`): `lang` (extension →
//! tree-sitter grammar) and `walker` (gitignore-honest file discovery).
//! Phase 0b adds the code-intelligence core (`langcfg`, `langmod`,
//! `codegraph`, `coderesolve`) here.

pub mod lang;
pub mod walker;

// Phase 0b code-intelligence core (E3): one imperative walk → semantic
// scope-qualified identity → scope/import resolution. No `.scm`.
pub mod codegraph;
pub mod coderesolve;
pub mod langcfg;
pub mod langmod;
