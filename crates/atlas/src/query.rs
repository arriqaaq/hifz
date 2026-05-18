//! Minimal corpus query: hybrid BM25 (node labels + chunk content) — the
//! atlas graph's text entry point. Vector ranking reuses hifz-core's
//! embedder when a node/chunk has an embedding.

use anyhow::Result;
use kernel::ids::rid_to_string;
use surrealdb::types::{RecordId, SurrealValue};

use crate::store::Store;

#[derive(Debug, serde::Serialize)]
pub struct Hit {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub snippet: Option<String>,
}

#[derive(Debug, SurrealValue)]
struct NodeHit {
    id: RecordId,
    kind: Option<String>,
    label: Option<String>,
    summary: Option<String>,
}
#[derive(Debug, SurrealValue)]
struct ChunkHit {
    node: RecordId,
    content: Option<String>,
}

pub async fn query(store: &Store, q: &str, limit: usize) -> Result<Vec<Hit>> {
    let p = store.project.clone();
    let mut out = Vec::new();

    let mut nr = store
        .db
        .query(
            "SELECT id, kind, label, summary FROM atlas_node \
             WHERE project=$p AND label @@ $q LIMIT $l",
        )
        .bind(("p", p.clone()))
        .bind(("q", q.to_string()))
        .bind(("l", limit as i64))
        .await?;
    for h in nr.take::<Vec<NodeHit>>(0).unwrap_or_default() {
        out.push(Hit {
            id: rid_to_string(&h.id),
            kind: h.kind.unwrap_or_default(),
            label: h.label.unwrap_or_default(),
            snippet: h.summary,
        });
    }

    let mut cr = store
        .db
        .query(
            "SELECT node, content FROM atlas_chunk \
             WHERE project=$p AND content @@ $q LIMIT $l",
        )
        .bind(("p", p))
        .bind(("q", q.to_string()))
        .bind(("l", limit as i64))
        .await?;
    for c in cr.take::<Vec<ChunkHit>>(0).unwrap_or_default() {
        out.push(Hit {
            id: rid_to_string(&c.node),
            kind: "chunk".into(),
            label: String::new(),
            snippet: c.content.map(|s| s.chars().take(200).collect()),
        });
    }
    out.truncate(limit);
    Ok(out)
}
