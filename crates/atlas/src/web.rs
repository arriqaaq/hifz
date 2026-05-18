//! atlas REST surface — an `axum::Router` the hifz daemon nests at
//! `/api/v1/atlas` behind the `atlas` feature (and the standalone CLI can
//! also serve). Thin handlers over the pipeline fns.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{get, post},
};
use hifz_core::db::Db;
use hifz_core::embed::Embedder;
use serde::Deserialize;
use serde_json::{Value, json};
use surrealdb::Surreal;

use crate::store::Store;

#[derive(Clone)]
pub struct AtlasState {
    pub db: Surreal<Db>,
    pub embedder: Arc<Embedder>,
    pub project: String,
}

impl AtlasState {
    fn store(&self) -> Store {
        Store::new(self.db.clone(), self.project.clone())
    }
}

pub fn router(state: AtlasState) -> Router {
    Router::new()
        .route("/ingest", post(ingest))
        .route("/code", post(code))
        .route("/extract", post(extract))
        .route("/cluster", post(cluster))
        .route("/insights", get(insights))
        .route("/graph", get(graph))
        .route("/query", get(query))
        .with_state(state)
}

fn err(e: anyhow::Error) -> Json<Value> {
    Json(json!({ "error": e.to_string() }))
}

#[derive(Deserialize)]
struct PathReq {
    path: String,
}

async fn ingest(State(s): State<AtlasState>, Json(r): Json<PathReq>) -> Json<Value> {
    match crate::ingest::ingest_path(&s.store(), &s.embedder, std::path::Path::new(&r.path)).await {
        Ok(rep) => Json(json!(rep)),
        Err(e) => err(e),
    }
}

async fn code(State(s): State<AtlasState>, Json(r): Json<PathReq>) -> Json<Value> {
    match crate::code::project_code_graph(&s.store(), &s.embedder, std::path::Path::new(&r.path))
        .await
    {
        Ok(rep) => Json(json!(rep)),
        Err(e) => err(e),
    }
}

async fn extract(State(s): State<AtlasState>) -> Json<Value> {
    let backend = crate::llm::LlmBackend::from_env();
    match crate::extract::extract_concepts(&s.store(), &s.embedder, backend.as_ref()).await {
        Ok(rep) => Json(json!(rep)),
        Err(e) => err(e),
    }
}

async fn cluster(State(s): State<AtlasState>) -> Json<Value> {
    match crate::cluster::cluster(&s.store()).await {
        Ok(rep) => Json(json!(rep)),
        Err(e) => err(e),
    }
}

async fn insights(State(s): State<AtlasState>) -> Json<Value> {
    match crate::analyze::analyze(&s.store()).await {
        Ok(ins) => Json(json!(ins)),
        Err(e) => err(e),
    }
}

#[derive(Deserialize)]
struct Q {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
}

async fn query(State(s): State<AtlasState>, Query(q): Query<Q>) -> Json<Value> {
    match crate::query::query(&s.store(), &q.q, q.limit.unwrap_or(20)).await {
        Ok(hits) => Json(json!({ "hits": hits })),
        Err(e) => err(e),
    }
}

/// Node+edge dump for the UI (Cytoscape-friendly).
async fn graph(State(s): State<AtlasState>) -> Json<Value> {
    let db = &s.db;
    let p = s.project.clone();
    let nodes = db
        .query("SELECT id, kind, label, cluster FROM atlas_node WHERE project=$p LIMIT 4000")
        .bind(("p", p.clone()))
        .await
        .and_then(|mut r| r.take::<Vec<Value>>(0))
        .unwrap_or_default();
    let edges = db
        .query(
            "SELECT in, out, relation, resolution, score FROM atlas_edge \
             WHERE in IN (SELECT VALUE id FROM atlas_node WHERE project=$p) LIMIT 8000",
        )
        .bind(("p", p))
        .await
        .and_then(|mut r| r.take::<Vec<Value>>(0))
        .unwrap_or_default();
    Json(json!({ "nodes": nodes, "edges": edges }))
}
