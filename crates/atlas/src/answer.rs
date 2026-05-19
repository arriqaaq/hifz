//! Query-time RAG: retrieve (Phase-1 ranked hybrid `query`) → ask the LLM to
//! answer **only from the numbered sources, citing `[n]` inline** → return
//! prose + structured citations. The UI's "Ask" calls this; Claude uses
//! `atlas_query` directly and synthesizes itself.
//!
//! Degrade-never-fail: with no LLM backend configured we still return the
//! ranked evidence as citations + a `note`, so the UI shows sources instead
//! of a blank box (mirrors extract.rs's no-LLM fallback philosophy).

use anyhow::Result;
use kernel::embed::Embedder;
use serde::Serialize;

use crate::llm::LlmBackend;
use crate::query::{Hit, query};
use crate::store::Store;

/// One cited source, source-agnostic (file now, Notion/Jira/Slack later —
/// same shape, the UI renders `source_uri` as copy-path or link by kind).
#[derive(Debug, Serialize)]
pub struct Citation {
    pub n: usize,
    pub doc_label: String,
    pub source_kind: Option<String>,
    pub source_uri: Option<String>,
    pub source_ref: Option<String>,
    pub location: Option<String>,
    pub snippet: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AnswerResult {
    /// Prose answer with inline `[n]` citations. Empty when degraded.
    pub answer: String,
    pub citations: Vec<Citation>,
    /// Set when degraded (no LLM) or no sources — UI shows it as a banner.
    pub note: Option<String>,
}

const SYS: &str = "You answer the user's question using ONLY the numbered \
sources provided. Cite every claim inline as [n] with the source's number. \
If the sources do not contain the answer, say so plainly. Be concise; do \
not invent sources or facts.";

/// Max sources fed to the LLM / returned as citations (prompt-size bound).
const MAX_SOURCES: usize = 8;

fn to_citations(hits: &[Hit]) -> Vec<Citation> {
    hits.iter()
        .take(MAX_SOURCES)
        .enumerate()
        .map(|(i, h)| Citation {
            n: i + 1,
            doc_label: h.doc_label.clone(),
            source_kind: h.source_kind.clone(),
            source_uri: h.source_uri.clone(),
            source_ref: h.source_ref.clone(),
            location: h.location.clone(),
            snippet: h.snippet.clone(),
        })
        .collect()
}

pub async fn answer(
    store: &Store,
    embedder: &Embedder,
    llm: Option<&LlmBackend>,
    q: &str,
    limit: usize,
) -> Result<AnswerResult> {
    let hits = query(store, embedder, q, limit).await?;
    let citations = to_citations(&hits);

    if citations.is_empty() {
        return Ok(AnswerResult {
            answer: String::new(),
            citations,
            note: Some("No relevant sources found in the atlas corpus.".into()),
        });
    }

    let Some(llm) = llm else {
        return Ok(AnswerResult {
            answer: String::new(),
            citations,
            note: Some(
                "No answer model configured (set OLLAMA_URL or ATLAS_LLM) — \
                 showing ranked sources only."
                    .into(),
            ),
        });
    };

    let mut ctx = String::new();
    for c in &citations {
        let loc = c.location.as_deref().unwrap_or("");
        let body = c.snippet.as_deref().unwrap_or("");
        ctx.push_str(&format!(
            "[{}] {} — {} {}\n{}\n\n",
            c.n,
            c.doc_label,
            c.source_ref.as_deref().unwrap_or(""),
            loc,
            body
        ));
    }
    let user = format!("Question: {q}\n\nSources:\n{ctx}");

    match llm.complete(SYS, &user).await {
        Ok(a) => Ok(AnswerResult {
            answer: a.trim().to_string(),
            citations,
            note: None,
        }),
        Err(e) => Ok(AnswerResult {
            answer: String::new(),
            citations,
            note: Some(format!(
                "Answer model failed ({e}) — showing ranked sources only."
            )),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    async fn seed(store: &Store, emb: &Embedder) {
        let id = "atlas_node:s1".to_string();
        let body = "The service mesh uses mutual TLS between all pods.";
        store
            .db
            .query(
                "UPSERT type::record($id) SET project=$p, kind='document', \
                 label='mesh.md', path='docs/mesh.md', summary=$s, embedding=$e, \
                 source_kind='pdf', source_uri='file:///abs/docs/mesh.md', \
                 source_ref='docs/mesh.md', cluster=-1, created_at='2026-01-01'",
            )
            .bind(("id", id.clone()))
            .bind(("p", store.project.clone()))
            .bind(("s", body.to_string()))
            .bind(("e", emb.embed_single(body).ok()))
            .await
            .unwrap()
            .check()
            .unwrap();
        store
            .db
            .query(
                "CREATE atlas_chunk SET node=type::record($id), project=$p, \
                 chunk_index=0, content=$c, embedding=$e, created_at='2026-01-01'",
            )
            .bind(("id", id))
            .bind(("p", store.project.clone()))
            .bind(("c", body.to_string()))
            .bind(("e", emb.embed_single(body).ok()))
            .await
            .unwrap()
            .check()
            .unwrap();
    }

    #[tokio::test]
    async fn stub_llm_answers_with_citations() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        seed(&store, &emb).await;

        let stub = LlmBackend::Stub("Pods talk over mutual TLS [1].".into());
        let r = answer(&store, &emb, Some(&stub), "how do pods communicate?", 10)
            .await
            .unwrap();
        assert_eq!(r.answer, "Pods talk over mutual TLS [1].");
        assert!(r.note.is_none(), "no degrade note when LLM present");
        assert!(!r.citations.is_empty(), "citations attached");
        let c = &r.citations[0];
        assert_eq!(c.n, 1);
        assert_eq!(c.doc_label, "mesh.md");
        assert_eq!(c.source_uri.as_deref(), Some("file:///abs/docs/mesh.md"));
    }

    #[tokio::test]
    async fn no_llm_degrades_to_ranked_sources() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        seed(&store, &emb).await;

        let r = answer(&store, &emb, None, "mutual TLS", 10).await.unwrap();
        assert!(r.answer.is_empty(), "no prose without a model");
        assert!(!r.citations.is_empty(), "but ranked sources still returned");
        assert!(
            r.note.as_deref().unwrap_or("").contains("No answer model"),
            "degrade note explains why"
        );
    }

    #[tokio::test]
    async fn no_sources_is_noted_not_failed() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();
        let r = answer(&store, &emb, None, "anything", 10).await.unwrap();
        assert!(r.citations.is_empty());
        assert!(
            r.note
                .as_deref()
                .unwrap_or("")
                .contains("No relevant sources")
        );
    }
}
