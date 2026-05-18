//! hifz-core — shared primitives extracted from the `hifz` crate so both
//! `hifz` and `atlas` can depend on them without a dependency cycle.
//!
//! Dependency direction: `hifz-core ← atlas ← hifz`. hifz-core has NO
//! intra-`hifz` deps (verified at extraction time). The `hifz` crate
//! re-exports every module here (`pub use hifz_core::db;` …) so existing
//! `crate::db::*` / external `hifz::db::*` paths resolve unchanged.

pub mod config;
pub mod db;
pub mod embed;
pub mod ids;
pub mod models;
pub mod ollama;
pub mod text;

#[cfg(feature = "code-parse")]
pub mod code_parse;
