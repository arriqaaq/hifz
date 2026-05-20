//! Corpus stats over the clustered atlas graph: node / edge / cluster counts
//! for the Atlas page's stat cards. Typed `Insights` (Serialize → JSON).
//!
//! The earlier hub-node / surprising-link / isolated-node analytics were
//! removed (display dropped from the UI; the listings added no value) — only
//! the three headline counts remain.

use std::collections::HashSet;

use anyhow::Result;
use surrealdb::types::SurrealValue;

use crate::store::Store;

#[derive(Debug, serde::Serialize)]
pub struct Insights {
    pub nodes: usize,
    pub edges: usize,
    pub clusters: usize,
}

#[derive(Debug, SurrealValue)]
struct ClusterRow {
    cluster: Option<i64>,
}
#[derive(Debug, SurrealValue)]
struct CountRow {
    c: Option<i64>,
}

pub async fn analyze(store: &Store) -> Result<Insights> {
    let pid = store.pid();
    // Node count + distinct (non -1) clusters in one scan of the cluster column.
    let mut nr = store
        .db
        .query("SELECT cluster FROM atlas_node WHERE project=$p")
        .bind(("p", pid.clone()))
        .await?;
    let node_rows: Vec<ClusterRow> = nr.take(0).unwrap_or_default();
    let nodes = node_rows.len();
    let clusters = node_rows
        .iter()
        .filter_map(|r| r.cluster)
        .filter(|&c| c >= 0)
        .collect::<HashSet<_>>()
        .len();

    let mut er = store
        .db
        .query("SELECT count() AS c FROM atlas_edge WHERE project=$p GROUP ALL")
        .bind(("p", pid))
        .await?;
    let edges = er
        .take::<Vec<CountRow>>(0)
        .unwrap_or_default()
        .into_iter()
        .next()
        .and_then(|r| r.c)
        .unwrap_or(0) as usize;

    Ok(Insights {
        nodes,
        edges,
        clusters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::types::RecordId;

    async fn mk(store: &Store, n: &str, c: i64) {
        store
            .db
            .query("UPSERT type::record($id) SET project=project:demo, kind='concept', label=$l, cluster=$c, created_at='2026-01-01'")
            .bind(("id", format!("atlas_node:{n}")))
            .bind(("l", n.to_string()))
            .bind(("c", c))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    async fn rel(store: &Store, a: &str, b: &str) {
        store
            .db
            .query("RELATE $a->atlas_edge->$b SET project=project:demo, relation='related', via='t', score=1.0, created_at='2026-01-01'")
            .bind(("a", RecordId::new("atlas_node", a.to_string())))
            .bind(("b", RecordId::new("atlas_node", b.to_string())))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn insights_count_nodes_edges_clusters() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        for (n, c) in [("hub", 0), ("x1", 0), ("x2", 0), ("y1", 1), ("orphan", 1)] {
            mk(&store, n, c).await;
        }
        rel(&store, "hub", "x1").await;
        rel(&store, "hub", "x2").await;
        rel(&store, "hub", "y1").await;

        let ins = analyze(&store).await.unwrap();
        assert_eq!(ins.nodes, 5, "5 nodes");
        assert_eq!(ins.edges, 3, "3 edges");
        assert_eq!(ins.clusters, 2, "clusters 0 and 1");
        // Serializes to JSON with only the three headline counts.
        let j = serde_json::to_value(&ins).unwrap();
        assert!(
            j.get("nodes").is_some() && j.get("edges").is_some() && j.get("clusters").is_some()
        );
        assert!(j.get("hub_nodes").is_none(), "hub_nodes removed");
        assert!(
            j.get("surprising_links").is_none(),
            "surprising_links removed"
        );
        assert!(j.get("isolated_nodes").is_none(), "isolated_nodes removed");
    }
}
