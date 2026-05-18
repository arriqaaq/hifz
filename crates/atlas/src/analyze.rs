//! Corpus analytics over the clustered atlas graph. Typed `Insights`
//! (Serialize → JSON; no markdown report, per the locked scope).
//!
//! These metrics are **inspired by graphify but re-derived** — graphify
//! uses raw degree, an integer additive surprise score with no Jaccard,
//! and has no isolated-node metric. The formulas here are atlas-original
//! (weighted degree + cluster-span; cross-cluster × endpoint-rarity ×
//! inverse-confidence; degree≤1 / island clusters). Stated honestly.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use hifz_core::ids::rid_to_string;
use surrealdb::types::{RecordId, SurrealValue};

use crate::store::Store;

const TOP_N: usize = 15;

#[derive(Debug, serde::Serialize)]
pub struct HubNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub weighted_degree: f64,
    pub clusters_touched: usize,
}
#[derive(Debug, serde::Serialize)]
pub struct SurprisingLink {
    pub from: String,
    pub to: String,
    pub relation: String,
    pub score: f64,
    pub surprise: f64,
    pub why: String,
}
#[derive(Debug, serde::Serialize)]
pub struct IsolatedNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub weighted_degree: f64,
}
#[derive(Debug, serde::Serialize)]
pub struct Insights {
    pub nodes: usize,
    pub edges: usize,
    pub clusters: usize,
    pub hub_nodes: Vec<HubNode>,
    pub surprising_links: Vec<SurprisingLink>,
    pub isolated_nodes: Vec<IsolatedNode>,
}

#[derive(Debug, SurrealValue)]
struct N {
    id: RecordId,
    label: Option<String>,
    kind: Option<String>,
    cluster: Option<i64>,
}
#[derive(Debug, SurrealValue)]
struct E {
    r#in: RecordId,
    out: RecordId,
    relation: Option<String>,
    score: Option<f64>,
}

pub async fn analyze(store: &Store) -> Result<Insights> {
    let p = store.project.clone();
    let mut nr = store
        .db
        .query("SELECT id, label, kind, cluster FROM atlas_node WHERE project=$p")
        .bind(("p", p.clone()))
        .await?;
    let nodes: Vec<N> = nr.take(0).unwrap_or_default();
    let mut er = store
        .db
        .query(
            "SELECT in, out, relation, score FROM atlas_edge WHERE \
             in IN (SELECT VALUE id FROM atlas_node WHERE project=$p)",
        )
        .bind(("p", p.clone()))
        .await?;
    let edges: Vec<E> = er.take(0).unwrap_or_default();

    let meta: HashMap<String, (&N,)> = nodes.iter().map(|n| (rid_to_string(&n.id), (n,))).collect();
    let label = |id: &str| {
        meta.get(id)
            .and_then(|(n,)| n.label.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let cluster_of = |id: &str| meta.get(id).and_then(|(n,)| n.cluster).unwrap_or(-1);
    let kind_of = |id: &str| {
        meta.get(id)
            .and_then(|(n,)| n.kind.clone())
            .unwrap_or_default()
    };

    let mut wdeg: HashMap<String, f64> = HashMap::new();
    let mut neigh_clusters: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut rel_freq: HashMap<String, usize> = HashMap::new();
    for e in &edges {
        let (a, b) = (rid_to_string(&e.r#in), rid_to_string(&e.out));
        let w = e.score.unwrap_or(1.0);
        *wdeg.entry(a.clone()).or_insert(0.0) += w;
        *wdeg.entry(b.clone()).or_insert(0.0) += w;
        neigh_clusters
            .entry(a.clone())
            .or_default()
            .insert(cluster_of(&b));
        neigh_clusters
            .entry(b.clone())
            .or_default()
            .insert(cluster_of(&a));
        *rel_freq
            .entry(e.relation.clone().unwrap_or_default())
            .or_insert(0) += 1;
    }

    // Hub nodes: weighted degree × cluster span.
    let mut hubs: Vec<HubNode> = nodes
        .iter()
        .map(|n| {
            let id = rid_to_string(&n.id);
            HubNode {
                label: n.label.clone().unwrap_or_else(|| id.clone()),
                kind: n.kind.clone().unwrap_or_default(),
                weighted_degree: *wdeg.get(&id).unwrap_or(&0.0),
                clusters_touched: neigh_clusters.get(&id).map(|s| s.len()).unwrap_or(0),
                id,
            }
        })
        .collect();
    hubs.sort_by(|a, b| {
        (b.weighted_degree, b.clusters_touched)
            .partial_cmp(&(a.weighted_degree, a.clusters_touched))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    hubs.truncate(TOP_N);

    // Surprising cross-cluster links: cross-cluster × endpoint rarity ×
    // inverse confidence × relation rarity.
    let mut surprising: Vec<SurprisingLink> = edges
        .iter()
        .filter_map(|e| {
            let (a, b) = (rid_to_string(&e.r#in), rid_to_string(&e.out));
            let (ca, cb) = (cluster_of(&a), cluster_of(&b));
            if ca == cb || ca < 0 || cb < 0 {
                return None;
            }
            let rel = e.relation.clone().unwrap_or_default();
            let sc = e.score.unwrap_or(1.0).max(0.01);
            let rarity_a = 1.0 / (wdeg.get(&a).copied().unwrap_or(1.0)).max(1.0);
            let rarity_b = 1.0 / (wdeg.get(&b).copied().unwrap_or(1.0)).max(1.0);
            let rel_rarity = 1.0 / (*rel_freq.get(&rel).unwrap_or(&1) as f64);
            let surprise = (rarity_a + rarity_b) * rel_rarity / sc;
            Some(SurprisingLink {
                why: format!(
                    "cross-cluster {}→{} via '{rel}' (rare endpoints, conf {sc:.2})",
                    ca, cb
                ),
                from: label(&a),
                to: label(&b),
                relation: rel,
                score: sc,
                surprise,
            })
        })
        .collect();
    surprising.sort_by(|x, y| {
        y.surprise
            .partial_cmp(&x.surprise)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(x.from.cmp(&y.from))
    });
    surprising.truncate(TOP_N);

    // Isolated: weighted degree ≤ 1 (orphan doc / dead code / missing link).
    let mut isolated: Vec<IsolatedNode> = nodes
        .iter()
        .filter_map(|n| {
            let id = rid_to_string(&n.id);
            let d = *wdeg.get(&id).unwrap_or(&0.0);
            if d <= 1.0 {
                Some(IsolatedNode {
                    label: n.label.clone().unwrap_or_else(|| id.clone()),
                    kind: n.kind.clone().unwrap_or_default(),
                    weighted_degree: d,
                    id,
                })
            } else {
                None
            }
        })
        .collect();
    isolated.sort_by(|a, b| a.id.cmp(&b.id));
    isolated.truncate(TOP_N * 2);

    let clusters = nodes
        .iter()
        .filter_map(|n| n.cluster)
        .filter(|&c| c >= 0)
        .collect::<HashSet<_>>()
        .len();

    let _ = kind_of; // (kept for future per-kind weighting)
    Ok(Insights {
        nodes: nodes.len(),
        edges: edges.len(),
        clusters,
        hub_nodes: hubs,
        surprising_links: surprising,
        isolated_nodes: isolated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mk(store: &Store, n: &str, c: i64) {
        store
            .db
            .query("UPSERT type::record($id) SET project='demo', kind='concept', label=$l, cluster=$c, created_at='2026-01-01'")
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
            .query("RELATE $a->atlas_edge->$b SET relation='related', via='t', score=1.0, created_at='2026-01-01'")
            .bind(("a", RecordId::new("atlas_node", a.to_string())))
            .bind(("b", RecordId::new("atlas_node", b.to_string())))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn insights_surface_hub_and_isolated() {
        let db = hifz_core::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        for (n, c) in [("hub", 0), ("x1", 0), ("x2", 0), ("y1", 1), ("orphan", 1)] {
            mk(&store, n, c).await;
        }
        rel(&store, "hub", "x1").await;
        rel(&store, "hub", "x2").await;
        rel(&store, "hub", "y1").await; // cross-cluster (0→1)

        let ins = analyze(&store).await.unwrap();
        assert_eq!(ins.nodes, 5);
        assert_eq!(ins.hub_nodes.first().map(|h| h.label.as_str()), Some("hub"));
        assert!(ins.hub_nodes[0].clusters_touched >= 2, "hub spans clusters");
        assert!(
            ins.surprising_links.iter().any(|s| s.relation == "related"),
            "cross-cluster link surfaced"
        );
        assert!(
            ins.isolated_nodes.iter().any(|i| i.label == "orphan"),
            "orphan flagged isolated"
        );
        // Serializes to JSON (no markdown).
        let j = serde_json::to_value(&ins).unwrap();
        assert!(j.get("hub_nodes").is_some());
    }
}
