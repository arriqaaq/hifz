//! atlas — a corpus knowledge graph built on hifz.
//!
//! Ingests code + docs + PDFs into a clustered graph with analytics,
//! riding hifz's living/grounded substrate (hifz-core: db, embed, the
//! code-intelligence core). atlas owns its own SurrealDB tables
//! (`atlas_node` / `atlas_edge` / `atlas_chunk`) in the *same* instance —
//! it only ever reads hifz's `code_symbol`/`edge`, never writes them.

pub mod analyze;
pub mod answer;
pub mod cluster;
pub mod code;
pub mod extract;
pub mod ingest;
pub mod llm;
pub mod query;
pub mod store;
pub mod web;

pub use store::{Store, init_atlas_schema};
