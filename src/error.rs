//! Typed client-error markers.
//!
//! The library keeps returning `anyhow::Result` everywhere (so `?` continues to
//! absorb arbitrary foreign errors — surrealdb, serde_json, io — via anyhow's
//! blanket `From`). A custom enum cannot replicate that blanket `From` without
//! violating coherence, so instead the few *deliberate* client-error conditions
//! wrap a `HifzError` value inside `anyhow::Error`. The web boundary recovers it
//! with `downcast_ref::<HifzError>()` and maps the variant to an HTTP status —
//! classification by type, never by string matching.
//!
//! Genuine internal failures stay as plain `anyhow::Error` and map to 500.

use thiserror::Error;

/// Deliberate client-error conditions the library raises. Anything *not* one of
/// these is treated as an internal (500) error at the web boundary.
#[derive(Debug, Error)]
pub enum HifzError {
    /// The requested entity does not exist → HTTP 404.
    #[error("{0}")]
    NotFound(String),

    /// The request was structurally accepted but semantically invalid → HTTP 400.
    #[error("{0}")]
    InvalidInput(String),
}
