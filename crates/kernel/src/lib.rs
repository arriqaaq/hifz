//! hifz-core — shared primitives extracted from the `hifz` crate so both
//! `hifz` and `maktab` can depend on them without a dependency cycle.
//!
//! Dependency direction: `hifz-core ← maktab ← hifz`. hifz-core has NO
//! intra-`hifz` deps (verified at extraction time). The `hifz` crate
//! re-exports every module here (`pub use kernel::db;` …) so existing
//! `crate::db::*` / external `hifz::db::*` paths resolve unchanged.

pub mod config;
pub mod db;
pub mod embed;
pub mod ids;
pub mod models;
pub mod ollama;
pub mod text;

pub mod code_parse;
