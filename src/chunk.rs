//! Long-form artifact chunking for retrieval.
//!
//! When a memory has `content_long` set (Plan, Design, CodeReview,
//! ShipReport, ContextSlice), this module splits it into overlapping chunks
//! and writes them to the `memory_chunk` table. Search hits chunks via
//! vector + BM25; retrieval groups hits by parent memory.
//!
//! ## Splitting strategy
//!
//! The splitter is structure-aware: it prefers boundaries at headings
//! (`#`, `##`), code-fence delimiters (` ``` `), and blank lines, then
//! falls back to token windows. This keeps semantic units intact more
//! often than a pure token-window split would.
//!
//! Token estimation uses a simple character-count heuristic
//! (`chars / 4 ≈ tokens`) — close enough for chunk sizing without
//! pulling in a tokenizer. The targets are:
//!
//! - chunk size: ~500 tokens (~2KB of text)
//! - overlap:    ~100 tokens (~400 chars)
//!
//! Both tunable via constants. The overlap matters: it gives the embedder
//! enough context to disambiguate phrases that straddle the boundary.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::embed::Embedder;
use crate::link;

// The pure splitter (`Chunk`, `split`, `best_boundary`,
// `snap_to_char_boundary`, size constants) moved to `kernel::text`.
// Re-exported so `crate::chunk::Chunk` / `crate::chunk::split` resolve
// unchanged. The DB writers below stay here (they need `Db`/`Embedder`).
pub use kernel::text::{Chunk, split};

// ---------------------------------------------------------------------------
// DB writers
// ---------------------------------------------------------------------------

/// Persist chunks for a parent memory: writes one `memory_chunk` row per
/// chunk and one `part_of` edge per chunk → parent. Embeds each chunk
/// individually. Idempotent at insert level via the `chunk_parent +
/// chunk_index` access pattern (callers should delete prior chunks before
/// re-emitting if they're updating).
pub async fn persist_chunks(
    db: &Surreal<Db>,
    embedder: &Embedder,
    parent_id: &RecordId,
    project: &str,
    chunks: &[Chunk],
) -> Result<usize> {
    if chunks.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut written = 0usize;

    #[derive(Debug, SurrealValue)]
    struct Created {
        id: Option<RecordId>,
    }

    for c in chunks {
        let embedding = match embedder.embed_single(&c.content) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("chunk embed failed at idx {}: {e}", c.index);
                continue;
            }
        };
        let mut resp = match db
            .query(
                "CREATE memory_chunk SET \
                 parent_id = $pid, \
                 project = $project, \
                 chunk_index = $idx, \
                 content = $content, \
                 embedding = $embedding, \
                 created_at = $now \
                 RETURN id",
            )
            .bind(("pid", parent_id.clone()))
            .bind(("project", project.to_string()))
            .bind(("idx", c.index as i64))
            .bind(("content", c.content.clone()))
            .bind(("embedding", embedding))
            .bind(("now", now.clone()))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("chunk insert failed at idx {}: {e}", c.index);
                continue;
            }
        };
        let created: Vec<Created> = resp.take(0).unwrap_or_default();
        let Some(chunk_id) = created.into_iter().next().and_then(|x| x.id) else {
            continue;
        };

        // chunk --part_of--> parent memory. Type-pair table allows
        // Memory->Memory only for `part_of`; chunk's table is `memory_chunk`
        // which maps to RecordKind::Other, which short-circuits to permitted.
        // (Recorded as a followup for Phase 10: extend RecordKind to
        // include MemoryChunk so the constraint is precise.)
        let _ = link::upsert_edge(
            db,
            &chunk_id,
            parent_id,
            "part_of",
            "system",
            1.0,
            Some(&format!("chunk {} of long-form artifact", c.index)),
        )
        .await;
        written += 1;
    }

    Ok(written)
}

/// Delete all chunks (and associated `part_of` edges) for a parent memory.
/// Used when re-chunking after a `PUT /markdown` update.
pub async fn delete_chunks_for(db: &Surreal<Db>, parent_id: &RecordId) -> Result<()> {
    // Collect chunk ids first so we can drop edges by endpoint.
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query("SELECT id FROM memory_chunk WHERE parent_id = $pid")
        .bind(("pid", parent_id.clone()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    let ids: Vec<RecordId> = rows.into_iter().filter_map(|r| r.id).collect();

    if !ids.is_empty() {
        // Drop edges where either endpoint is one of these chunks.
        let _ = db
            .query("DELETE edge WHERE in IN $ids OR out IN $ids")
            .bind(("ids", ids.clone()))
            .await;
        let _ = db
            .query("DELETE memory_chunk WHERE id IN $ids")
            .bind(("ids", ids))
            .await;
    }
    Ok(())
}

// Splitter unit tests moved with the code to `hifz-core/src/text.rs`.
// `persist_chunks`/`delete_chunks_for` are exercised by the DB-backed
// integration tests.
