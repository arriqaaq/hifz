//! Concept-graph extraction from document prose.
//!
//! With an LLM backend: per chunk, ask for a strict-JSON `{nodes,edges}`
//! concept graph (one repair retry, then skip — never crash, never
//! fabricate). Without a backend: a deterministic no-LLM fallback links
//! semantically-similar document nodes (`related`, via `embedding`) —
//! nothing is dropped, no LLM required.
//!
//! Idempotent: every run first wipes this project's prior concept layer
//! (`via IN ['llm','embedding']` edges + `kind='concept'` nodes), then
//! recreates it — concept nodes via normalized-label deterministic id,
//! edges deduped within the run by (doc,concept) / (src,tgt,relation).
//! `RELATE` is an *insert* (not an upsert), so the wipe is what actually
//! guarantees re-runs don't accumulate duplicate edges.

use std::collections::HashSet;

use anyhow::Result;
use kernel::embed::Embedder;
use kernel::ids::rid_to_string;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use surrealdb::types::{RecordId, SurrealValue};

use crate::llm::{LlmBackend, strip_json_fence};
use crate::store::Store;

const MAX_CHUNKS_PER_DOC: usize = 6;
const FALLBACK_SIM: f32 = 0.82;

#[derive(Debug, Default, serde::Serialize)]
pub struct ExtractReport {
    pub concepts: usize,
    pub edges: usize,
    pub chunks_processed: usize,
    pub repaired: usize,
    pub skipped: usize,
    pub fallback_edges: usize,
}

#[derive(Debug, Deserialize, Default)]
struct LlmOut {
    #[serde(default)]
    nodes: Vec<LNode>,
    #[serde(default)]
    edges: Vec<LEdge>,
}
#[derive(Debug, Deserialize)]
struct LNode {
    label: String,
}
#[derive(Debug, Deserialize)]
struct LEdge {
    source: String,
    target: String,
    #[serde(default)]
    relation: String,
}

fn norm_label(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn concept_key(project: &str, label: &str) -> String {
    let mut h = Sha256::new();
    h.update(project.as_bytes());
    h.update(b"\0concept\0");
    h.update(norm_label(label).as_bytes());
    hex::encode(&h.finalize()[..16])
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

const SYS: &str = "You extract a concept graph from a document excerpt. \
Output STRICT JSON only, no prose, no markdown fences: \
{\"nodes\":[{\"label\":\"...\"}],\"edges\":[{\"source\":\"...\",\"target\":\"...\",\"relation\":\"...\"}]}. \
Labels are short domain concepts. relation is a short verb phrase.";
const SYS_REPAIR: &str = "Return ONLY a single valid JSON object of the form \
{\"nodes\":[{\"label\":\"...\"}],\"edges\":[{\"source\":\"...\",\"target\":\"...\",\"relation\":\"...\"}]}. \
No markdown. No commentary.";

#[derive(Debug, SurrealValue)]
struct DocRow {
    id: RecordId,
    embedding: Option<Vec<f32>>,
}
#[derive(Debug, SurrealValue)]
struct ChunkRow {
    content: Option<String>,
    #[allow(dead_code)]
    chunk_index: Option<i64>,
}

pub async fn extract_concepts(
    store: &Store,
    embedder: &Embedder,
    backend: Option<&LlmBackend>,
) -> Result<ExtractReport> {
    let mut report = ExtractReport::default();
    let now = chrono::Utc::now().to_rfc3339();

    // Idempotency (fix for the non-idempotent `RELATE`): wipe this project's
    // prior concept layer before recreating, exactly as code.rs:53-66 does for
    // the code layer. SurrealDB `RELATE` always *inserts* a new edge (it is
    // not an upsert), so without this every re-run duplicated every concept
    // edge — inflating `score`/weighted-degree and skewing cluster.rs +
    // analyze.rs. `via IN ['llm','embedding']` covers both the LLM concept
    // edges and the no-LLM `related` fallback; concept *nodes* are deleted so
    // a re-run can't leave orphans (they UPSERT back deterministically).
    let _ = store
        .db
        .query("DELETE atlas_edge WHERE project=$p AND via IN ['llm','embedding']")
        .bind(("p", store.project.clone()))
        .await;
    let _ = store
        .db
        .query("DELETE atlas_node WHERE project=$p AND kind='concept'")
        .bind(("p", store.project.clone()))
        .await;

    let mut dr = store
        .db
        .query("SELECT id, embedding FROM atlas_node WHERE project=$p AND kind='document'")
        .bind(("p", store.project.clone()))
        .await?;
    let docs: Vec<DocRow> = dr.take(0).unwrap_or_default();

    // -- No-LLM fallback: deterministic similarity edges between docs -----
    let Some(backend) = backend else {
        for i in 0..docs.len() {
            for j in (i + 1)..docs.len() {
                let (Some(a), Some(b)) = (&docs[i].embedding, &docs[j].embedding) else {
                    continue;
                };
                let sim = cosine(a, b);
                if sim >= FALLBACK_SIM {
                    let _ = store
                        .db
                        .query(
                            "RELATE $a->atlas_edge->$b SET project=$p, \
                             relation='related', via='embedding', score=$s, \
                             created_at=$now",
                        )
                        .bind(("a", docs[i].id.clone()))
                        .bind(("b", docs[j].id.clone()))
                        .bind(("p", store.project.clone()))
                        .bind(("s", sim as f64))
                        .bind(("now", now.clone()))
                        .await;
                    report.fallback_edges += 1;
                }
            }
        }
        return Ok(report);
    };

    // -- LLM concept extraction -----------------------------------------
    // Within-run dedupe (the second half of the B1 fix): a concept the LLM
    // names in multiple chunks/docs must be counted/UPSERTed once, a
    // (doc,concept) `mentions` edge created once, a (src,tgt,rel)
    // concept→concept edge created once — otherwise a doc whose N chunks each
    // mention "JWT" produced N identical edges in a single pass.
    let mut seen_concept: HashSet<String> = HashSet::new();
    let mut seen_mention: HashSet<(String, String)> = HashSet::new();
    let mut seen_cc: HashSet<(String, String, String)> = HashSet::new();
    for doc in &docs {
        let doc_key = rid_to_string(&doc.id);
        let mut cr = store
            .db
            .query(
                "SELECT content, chunk_index FROM atlas_chunk WHERE node=$n \
                 ORDER BY chunk_index LIMIT $lim",
            )
            .bind(("n", doc.id.clone()))
            .bind(("lim", MAX_CHUNKS_PER_DOC as i64))
            .await?;
        let chunks: Vec<ChunkRow> = cr.take(0).unwrap_or_default();

        for ch in chunks {
            let Some(content) = ch.content else { continue };
            let excerpt: String = content.chars().take(3000).collect();
            report.chunks_processed += 1;

            let raw = match backend.complete(SYS, &excerpt).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("llm complete failed: {e}");
                    report.skipped += 1;
                    continue;
                }
            };
            let parsed: Option<LlmOut> = serde_json::from_str(strip_json_fence(&raw)).ok();
            let out = match parsed {
                Some(o) => o,
                None => {
                    // one repair retry
                    match backend.complete(SYS_REPAIR, &excerpt).await {
                        Ok(r2) => match serde_json::from_str(strip_json_fence(&r2)) {
                            Ok(o) => {
                                report.repaired += 1;
                                o
                            }
                            Err(_) => {
                                report.skipped += 1;
                                continue;
                            }
                        },
                        Err(_) => {
                            report.skipped += 1;
                            continue;
                        }
                    }
                }
            };

            // Upsert concept nodes (dedupe via normalized-label id).
            let mut labels: Vec<String> = out.nodes.iter().map(|n| n.label.clone()).collect();
            for e in &out.edges {
                labels.push(e.source.clone());
                labels.push(e.target.clone());
            }
            for label in &labels {
                if label.trim().is_empty() {
                    continue;
                }
                let ckey = concept_key(&store.project, label);
                let crid = RecordId::new("atlas_node", ckey.clone());
                if seen_concept.insert(ckey.clone()) {
                    let emb = embedder.embed_single(label).ok();
                    let created = store
                        .db
                        .query(
                            "UPSERT type::record($id) SET project=$p, kind='concept', \
                             label=$l, embedding=$e, cluster=-1, created_at=$now",
                        )
                        .bind(("id", format!("atlas_node:{ckey}")))
                        .bind(("p", store.project.clone()))
                        .bind(("l", label.trim().to_string()))
                        .bind(("e", emb))
                        .bind(("now", now.clone()))
                        .await
                        .and_then(|r| r.check());
                    if created.is_ok() {
                        report.concepts += 1;
                    }
                }
                // document --mentions--> concept (once per (doc,concept))
                if seen_mention.insert((doc_key.clone(), ckey.clone())) {
                    let _ = store
                        .db
                        .query(
                            "RELATE $d->atlas_edge->$c SET project=$p, \
                             relation='mentions', via='llm', score=1.0, \
                             created_at=$now",
                        )
                        .bind(("d", doc.id.clone()))
                        .bind(("c", crid))
                        .bind(("p", store.project.clone()))
                        .bind(("now", now.clone()))
                        .await;
                }
            }
            // concept --relation--> concept (once per (src,tgt,relation))
            for e in &out.edges {
                if e.source.trim().is_empty() || e.target.trim().is_empty() {
                    continue;
                }
                let sk = concept_key(&store.project, &e.source);
                let tk = concept_key(&store.project, &e.target);
                let rel = if e.relation.trim().is_empty() {
                    "related".to_string()
                } else {
                    e.relation.trim().to_string()
                };
                if seen_cc.insert((sk.clone(), tk.clone(), rel.clone())) {
                    let _ = store
                        .db
                        .query(
                            "RELATE $s->atlas_edge->$t SET project=$p, \
                             relation=$r, via='llm', score=0.8, created_at=$now",
                        )
                        .bind(("s", RecordId::new("atlas_node", sk)))
                        .bind(("t", RecordId::new("atlas_node", tk)))
                        .bind(("p", store.project.clone()))
                        .bind(("r", rel))
                        .bind(("now", now.clone()))
                        .await;
                    report.edges += 1;
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    async fn seed_doc(store: &Store, emb: &Embedder, label: &str, body: &str) {
        let id = format!("atlas_node:doc_{}", label.replace(' ', "_"));
        let e = emb.embed_single(body).ok();
        store
            .db
            .query(
                "UPSERT type::record($id) SET project=$p, kind='document', \
                 label=$l, path=$l, embedding=$e, cluster=-1, created_at='2026-01-01'",
            )
            .bind(("id", id.clone()))
            .bind(("p", store.project.clone()))
            .bind(("l", label.to_string()))
            .bind(("e", e))
            .await
            .unwrap()
            .check()
            .unwrap();
        store
            .db
            .query("CREATE atlas_chunk SET node=type::record($id), project=$p, chunk_index=0, content=$c, created_at='2026-01-01'")
            .bind(("id", id))
            .bind(("p", store.project.clone()))
            .bind(("c", body.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
    }

    #[tokio::test]
    async fn stub_backend_extracts_and_dedupes() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        seed_doc(
            &store,
            &emb,
            "auth",
            "the auth flow issues a jwt and a session",
        )
        .await;

        let stub = LlmBackend::Stub(
            "{\"nodes\":[{\"label\":\"Auth Flow\"},{\"label\":\"JWT\"}],\
             \"edges\":[{\"source\":\"Auth Flow\",\"target\":\"JWT\",\"relation\":\"issues\"}]}"
                .into(),
        );
        let r = extract_concepts(&store, &emb, Some(&stub)).await.unwrap();
        assert!(r.edges >= 1 && r.concepts >= 2);

        #[derive(Debug, SurrealValue)]
        struct C {
            c: Option<i64>,
        }
        let cnt = |db: surrealdb::Surreal<kernel::db::Db>, sql: &'static str| async move {
            let mut q = db.query(sql).await.unwrap();
            let r: Vec<C> = q.take(0).unwrap_or_default();
            r.into_iter().next().and_then(|x| x.c).unwrap_or(0)
        };
        let concepts = cnt(
            store.db.clone(),
            "SELECT count() AS c FROM atlas_node WHERE kind='concept' GROUP ALL",
        )
        .await;
        assert_eq!(concepts, 2, "Auth Flow + JWT, deduped");
        let mentions = cnt(
            store.db.clone(),
            "SELECT count() AS c FROM atlas_edge WHERE relation='mentions' GROUP ALL",
        )
        .await;
        assert_eq!(mentions, 2, "document mentions both concepts, once each");
        let all_edges = cnt(
            store.db.clone(),
            "SELECT count() AS c FROM atlas_edge GROUP ALL",
        )
        .await;
        assert_eq!(all_edges, 3, "2 mentions + 1 concept→concept, no dup");

        // B1 regression guard: re-running extract must NOT accumulate edges.
        // (`RELATE` is an insert; the per-run wipe is what makes this hold.)
        for _ in 0..3 {
            extract_concepts(&store, &emb, Some(&stub)).await.unwrap();
        }
        let concepts2 = cnt(
            store.db.clone(),
            "SELECT count() AS c FROM atlas_node WHERE kind='concept' GROUP ALL",
        )
        .await;
        assert_eq!(concepts2, 2, "no concept duplication after 3 more runs");
        let edges2 = cnt(
            store.db.clone(),
            "SELECT count() AS c FROM atlas_edge GROUP ALL",
        )
        .await;
        assert_eq!(edges2, 3, "edge count STABLE across re-runs (B1 fixed)");
    }

    #[tokio::test]
    async fn no_backend_fallback_links_similar_docs() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        seed_doc(
            &store,
            &emb,
            "a",
            "authentication tokens sessions jwt login flow",
        )
        .await;
        seed_doc(
            &store,
            &emb,
            "b",
            "authentication tokens sessions jwt login flow",
        )
        .await;

        let r = extract_concepts(&store, &emb, None).await.unwrap();
        assert!(
            r.fallback_edges >= 1,
            "near-identical docs linked by embedding fallback (nothing dropped)"
        );
    }
}
