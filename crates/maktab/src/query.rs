//! Corpus retrieval: hybrid search over the maktab graph — vector KNN and
//! BM25, fused with SurrealDB's `search::rrf`, the same idiom hifz-core uses
//! (`src/search.rs` `search_hybrid_with_config`).
//!
//! Every branch is project-scoped. Document *content* lives in `maktab_chunk`;
//! topical labels/summaries live in `maktab_node`. We fuse five branches
//! across both tables in one `search::rrf` call so a strong chunk match
//! outranks a weak label match by reciprocal rank, then hydrate each fused
//! id back to its **source provenance** (the parent document's
//! `source_kind`/`source_uri`/`source_ref` trio + chunk location) so a hit
//! always answers "what came from where".

use anyhow::Result;
use kernel::embed::Embedder;
use kernel::ids::rid_to_string;
use kernel::models::RrfResult;
use surrealdb::types::{RecordId, SurrealValue};

use crate::store::Store;

/// One ranked, provenance-carrying hit. No filesystem-typed field — only the
/// source-agnostic trio, so a future Slack/Jira/Notion connector needs zero
/// API change (it just writes a URL into `source_uri`).
#[derive(Debug, serde::Serialize)]
pub struct Hit {
    /// Parent document/node record id (chunk hits resolve to their parent).
    pub id: String,
    /// `document` | `concept` | `code_symbol` | `external` | `file`.
    pub kind: String,
    /// Human title — filename now, page/issue title for a future connector.
    pub doc_label: String,
    /// `file` | `pdf` | `code` | `concept` now; `notion`/`slack`/… later.
    pub source_kind: Option<String>,
    /// The one openable locator (`file://…` now, `https://…` later).
    pub source_uri: Option<String>,
    /// Human breadcrumb shown as the citation text.
    pub source_ref: Option<String>,
    /// Opaque within-source pointer (`"chunk 4"` now; `"p.12"`/block-id later).
    pub location: Option<String>,
    /// Matched passage / summary excerpt (the best chunk's, for this doc).
    pub snippet: Option<String>,
    /// Fused RRF score (higher = more relevant). Results are sorted by it.
    pub score: f64,
    /// Document-level dedup (B1): how many chunks of this document matched.
    pub chunk_count: usize,
    /// The contributing chunks, best-first (top-level fields mirror chunks[0]).
    pub chunks: Vec<ChunkRef>,
}

/// One contributing chunk of a deduplicated document hit.
#[derive(Debug, serde::Serialize, Clone)]
pub struct ChunkRef {
    pub location: Option<String>,
    pub snippet: Option<String>,
    pub score: f64,
}

#[derive(Debug, SurrealValue)]
struct ChunkRow {
    id: RecordId,
    node: RecordId,
    content: Option<String>,
    chunk_index: Option<i64>,
}
#[derive(Debug, SurrealValue)]
struct NodeRow {
    id: RecordId,
    kind: Option<String>,
    label: Option<String>,
    summary: Option<String>,
    source_kind: Option<String>,
    source_uri: Option<String>,
    source_ref: Option<String>,
}

const RRF_K: usize = 60;

/// Ranked hybrid retrieval. `embedder` embeds the query for the vector
/// branches (same local embedder ingest used, so dims match the HNSW index).
pub async fn query(store: &Store, embedder: &Embedder, q: &str, limit: usize) -> Result<Vec<Hit>> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let pid = store.pid();
    let qvec = embedder.embed_single(q)?;

    // Five RRF branches, all project-scoped. Chunk content carries the
    // answer text; node label/summary catch topical/title matches.
    let sql = format!(
        "search::rrf([\
             (SELECT id FROM maktab_chunk \
              WHERE project=$p AND embedding <|{limit},80|> $qv),\
             (SELECT id, search::score(1) AS ft FROM maktab_chunk \
              WHERE project=$p AND content @1,OR@ $q ORDER BY ft DESC LIMIT {limit}),\
             (SELECT id FROM maktab_node \
              WHERE project=$p AND embedding <|{limit},80|> $qv),\
             (SELECT id, search::score(2) AS ft FROM maktab_node \
              WHERE project=$p AND label @2,OR@ $q ORDER BY ft DESC LIMIT {limit}),\
             (SELECT id, search::score(3) AS ft FROM maktab_node \
              WHERE project=$p AND summary @3,OR@ $q ORDER BY ft DESC LIMIT {limit})\
         ], {limit}, {RRF_K})"
    );

    let mut resp = store
        .db
        .query(&sql)
        .bind(("p", pid.clone()))
        .bind(("q", q.to_string()))
        .bind(("qv", qvec))
        .await?;
    let mut fused: Vec<RrfResult> = resp.take(0).unwrap_or_default();
    if fused.is_empty() {
        return Ok(Vec::new());
    }
    // RRF returns fused rows; sort by score desc to be order-independent.
    fused.sort_by(|a, b| {
        b.rrf_score
            .unwrap_or(0.0)
            .partial_cmp(&a.rrf_score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                rid_to_string(a.id.as_ref().unwrap()).cmp(&rid_to_string(b.id.as_ref().unwrap()))
            })
    });

    // Split fused ids by table; chunk ids need their parent node for
    // provenance, node ids carry their own.
    let mut chunk_ids: Vec<RecordId> = Vec::new();
    let mut node_ids: Vec<RecordId> = Vec::new();
    for r in &fused {
        let Some(id) = &r.id else { continue };
        if rid_to_string(id).starts_with("maktab_chunk:") {
            chunk_ids.push(id.clone());
        } else {
            node_ids.push(id.clone());
        }
    }

    // Hydrate chunks, then collect their parent node ids for provenance.
    let mut chunk_by_id: std::collections::HashMap<String, ChunkRow> =
        std::collections::HashMap::new();
    if !chunk_ids.is_empty() {
        let mut cr = store
            .db
            .query("SELECT id, node, content, chunk_index FROM maktab_chunk WHERE id IN $ids")
            .bind(("ids", chunk_ids))
            .await?;
        for c in cr.take::<Vec<ChunkRow>>(0).unwrap_or_default() {
            node_ids.push(c.node.clone());
            chunk_by_id.insert(rid_to_string(&c.id), c);
        }
    }

    let mut node_by_id: std::collections::HashMap<String, NodeRow> =
        std::collections::HashMap::new();
    if !node_ids.is_empty() {
        let mut nr = store
            .db
            .query(
                "SELECT id, kind, label, summary, source_kind, source_uri, source_ref \
                 FROM maktab_node WHERE id IN $ids",
            )
            .bind(("ids", node_ids))
            .await?;
        for n in nr.take::<Vec<NodeRow>>(0).unwrap_or_default() {
            node_by_id.insert(rid_to_string(&n.id), n);
        }
    }

    let snip = |s: Option<String>| s.map(|t| t.chars().take(240).collect::<String>());
    // Document-level dedup (B1): one Hit per parent document. `fused` is sorted
    // by score desc, so the first chunk seen for a doc is its best — its
    // location/snippet/score become the Hit's top-level fields; later chunks of
    // the same doc append to `chunks` and bump `chunk_count`. `limit` now bounds
    // documents, not chunks. Inline `[n]` citations stay stable: one number per
    // document, the contributing chunk locations preserved in `chunks`.
    let mut out: Vec<Hit> = Vec::new();
    let mut doc_idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in &fused {
        let (Some(id), score) = (&r.id, r.rrf_score.unwrap_or(0.0)) else {
            continue;
        };
        let sid = rid_to_string(id);
        let (node, location, snippet): (&NodeRow, Option<String>, Option<String>) =
            if let Some(c) = chunk_by_id.get(&sid) {
                let Some(n) = node_by_id.get(&rid_to_string(&c.node)) else {
                    continue;
                };
                (
                    n,
                    Some(format!("chunk {}", c.chunk_index.unwrap_or(0))),
                    snip(c.content.clone()),
                )
            } else if let Some(n) = node_by_id.get(&sid) {
                (n, None, snip(n.summary.clone()))
            } else {
                continue;
            };
        let doc_id = rid_to_string(&node.id);
        let cref = ChunkRef {
            location: location.clone(),
            snippet: snippet.clone(),
            score,
        };
        if let Some(&i) = doc_idx.get(&doc_id) {
            // Already have this document — append the chunk, keep best fields.
            out[i].chunk_count += 1;
            out[i].chunks.push(cref);
            continue;
        }
        if out.len() >= limit {
            continue; // document budget reached; don't start a new doc
        }
        doc_idx.insert(doc_id.clone(), out.len());
        out.push(Hit {
            id: doc_id,
            kind: node.kind.clone().unwrap_or_default(),
            doc_label: node.label.clone().unwrap_or_default(),
            source_kind: node.source_kind.clone(),
            source_uri: node.source_uri.clone(),
            source_ref: node.source_ref.clone(),
            location,
            snippet,
            score,
            chunk_count: 1,
            chunks: vec![cref],
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(store: &Store, emb: &Embedder, key: &str, label: &str, body: &str) {
        let id = format!("maktab_node:{key}");
        let summary: String = body.chars().take(280).collect();
        let nemb = emb.embed_single(&format!("{label}\n{summary}")).ok();
        let pid = store.pid();
        store
            .db
            .query(
                "UPSERT type::record($id) SET project=$p, kind='document', label=$l, \
                 path=$r, summary=$s, embedding=$e, source_kind='pdf', \
                 source_uri=$u, source_ref=$r, cluster=-1, created_at='2026-01-01'",
            )
            .bind(("id", id.clone()))
            .bind(("p", pid.clone()))
            .bind(("l", label.to_string()))
            .bind(("r", format!("docs/{label}")))
            .bind(("s", summary))
            .bind(("e", nemb))
            .bind(("u", format!("file:///abs/docs/{label}")))
            .await
            .unwrap()
            .check()
            .unwrap();
        for ch in kernel::text::split(body) {
            let e = emb.embed_single(&ch.content).ok();
            store
                .db
                .query(
                    "CREATE maktab_chunk SET node=type::record($id), project=$p, \
                     chunk_index=$i, content=$c, embedding=$e, created_at='2026-01-01'",
                )
                .bind(("id", id.clone()))
                .bind(("p", pid.clone()))
                .bind(("i", ch.index as i64))
                .bind(("c", ch.content.clone()))
                .bind(("e", e))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn ranks_by_relevance_and_carries_provenance() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_maktab_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        seed(
            &store,
            &emb,
            "a",
            "auth.md",
            "The authentication flow issues a JWT access token and a refresh \
             token. Sessions expire after thirty minutes of inactivity.",
        )
        .await;
        seed(
            &store,
            &emb,
            "b",
            "billing.md",
            "Invoices are generated monthly. Late payments incur a fee after \
             a thirty day grace period.",
        )
        .await;

        // Keyword hit ranks + carries real provenance.
        let hits = query(&store, &emb, "JWT refresh token", 10).await.unwrap();
        assert!(!hits.is_empty(), "got hits");
        let top = &hits[0];
        assert_eq!(top.doc_label, "auth.md", "auth doc ranks first");
        assert_eq!(top.source_kind.as_deref(), Some("pdf"));
        assert_eq!(
            top.source_uri.as_deref(),
            Some("file:///abs/docs/auth.md"),
            "openable provenance present"
        );
        assert!(top.location.is_some(), "chunk location present");
        assert!(top.score > 0.0, "ranked by fused score");

        // B3 proof: a paraphrase with NO shared keywords still finds the doc
        // (pure BM25 would miss this — proves the vector branch is live).
        let sem = query(&store, &emb, "how long until a login expires", 10)
            .await
            .unwrap();
        assert!(
            sem.iter().any(|h| h.doc_label == "auth.md"),
            "semantic (vector) retrieval finds the session-expiry doc"
        );
    }

    #[tokio::test]
    async fn dedups_document_with_many_matching_chunks() {
        // B1 regression fence: a single document whose many chunks all match
        // must appear ONCE (one Hit), not once-per-chunk, with chunk_count
        // reflecting the matched chunks.
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_maktab_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        seed(
            &store,
            &emb,
            "big",
            "big.md",
            &"The authentication flow issues a JWT token and a session. ".repeat(200),
        )
        .await;

        let hits = query(&store, &emb, "JWT token session", 10).await.unwrap();
        let big: Vec<&Hit> = hits.iter().filter(|h| h.doc_label == "big.md").collect();
        assert_eq!(big.len(), 1, "document deduped to a single hit");
        assert!(
            big[0].chunk_count >= 1 && big[0].chunks.len() == big[0].chunk_count,
            "chunk_count tracks the contributing chunks ({} / {})",
            big[0].chunk_count,
            big[0].chunks.len()
        );
    }

    #[tokio::test]
    async fn empty_query_is_empty() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_maktab_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        assert!(query(&store, &emb, "   ", 10).await.unwrap().is_empty());
    }
}
