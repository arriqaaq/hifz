use std::collections::HashSet;

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, RecordIdKey, SurrealValue};

use crate::db::Db;
use crate::link::{self, Direction, GraphExpandConfig};

#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceNode {
    pub id: String,
    pub table: String,
    pub label: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
    pub via: String,
    pub score: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceResult {
    pub nodes: Vec<TraceNode>,
    pub edges: Vec<TraceEdge>,
}

pub async fn trace(
    db: &Surreal<Db>,
    start_id: &str,
    direction: &str,
    relations: Option<Vec<String>>,
    max_hops: usize,
) -> Result<TraceResult> {
    let rid = resolve_record_id(db, start_id).await?;

    let dir = match direction {
        "forward" => Direction::Outgoing,
        "backward" => Direction::Incoming,
        _ => Direction::Both,
    };

    let config = GraphExpandConfig {
        max_hops,
        relations,
        min_score: 0.0,
        dampening: 1.0,
        max_results: 100,
        direction: dir,
    };

    let edges = link::expand_graph(db, &[rid.clone()], &config).await?;

    let mut node_ids: HashSet<String> = HashSet::new();
    node_ids.insert(rid_to_string(&rid));

    let mut trace_edges = Vec::new();
    for e in &edges {
        let from_str = rid_to_string(&e.from);
        let to_str = rid_to_string(&e.to);
        node_ids.insert(from_str.clone());
        node_ids.insert(to_str.clone());
        trace_edges.push(TraceEdge {
            from: from_str,
            to: to_str,
            relation: e.relation.clone(),
            via: e.via.clone(),
            score: e.score,
        });
    }

    let mut nodes = Vec::new();
    for id_str in &node_ids {
        let node = fetch_node_info(db, id_str).await.unwrap_or_else(|_| {
            let table = id_str.split(':').next().unwrap_or("unknown").to_string();
            TraceNode {
                id: id_str.clone(),
                table,
                label: id_str.clone(),
                created_at: None,
            }
        });
        nodes.push(node);
    }

    Ok(TraceResult {
        nodes,
        edges: trace_edges,
    })
}

fn rid_to_string(rid: &RecordId) -> String {
    let key = match &rid.key {
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::Uuid(u) => u.to_string(),
        other => format!("{other:?}"),
    };
    format!("{}:{key}", rid.table)
}

async fn resolve_record_id(db: &Surreal<Db>, id_str: &str) -> Result<RecordId> {
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query("SELECT id FROM type::record($id)")
        .bind(("id", id_str.to_string()))
        .await?;
    let rows: Vec<Row> = resp.take(0)?;
    rows.into_iter()
        .next()
        .and_then(|r| r.id)
        .ok_or_else(|| anyhow::anyhow!("record not found: {id_str}"))
}

async fn fetch_node_info(db: &Surreal<Db>, id_str: &str) -> Result<TraceNode> {
    let table = id_str.split(':').next().unwrap_or("unknown").to_string();

    let mut resp = db
        .query("SELECT * FROM type::record($id)")
        .bind(("id", id_str.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = resp.take(0)?;

    if let Some(row) = rows.into_iter().next() {
        let label = row
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| row.get("prompt").and_then(|v| v.as_str()))
            .or_else(|| row.get("name").and_then(|v| v.as_str()))
            .or_else(|| row.get("sha").and_then(|v| v.as_str()))
            .or_else(|| row.get("fact").and_then(|v| v.as_str()))
            .unwrap_or(id_str)
            .to_string();
        let created_at = row
            .get("created_at")
            .and_then(|v| v.as_str())
            .or_else(|| row.get("started_at").and_then(|v| v.as_str()))
            .or_else(|| row.get("timestamp").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        Ok(TraceNode {
            id: id_str.to_string(),
            table,
            label,
            created_at,
        })
    } else {
        Err(anyhow::anyhow!("node not found: {id_str}"))
    }
}
