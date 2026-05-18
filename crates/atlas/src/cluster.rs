//! Modularity clustering (Louvain) over the atlas graph + graphify-style
//! refinement: re-split any cluster larger than 25% of nodes (min split
//! size 10). Deterministic (sorted node iteration). Writes `cluster` onto
//! `atlas_node`.
//!
//! NOTE (decision gate from the plan): maintained Rust Leiden crates exist
//! (`fa-leiden-cd`/`avila-clustering`); Louvain is used here for a
//! dependency-free, deterministic baseline. The modularity-quality delta
//! vs Leiden is the documented, bounded trade-off — swapping in a Leiden
//! crate is a localized change behind this module's surface.

use std::collections::HashMap;

use anyhow::Result;
use hifz_core::ids::rid_to_string;
use surrealdb::types::{RecordId, SurrealValue};

use crate::store::Store;

const MAX_CLUSTER_FRACTION: f64 = 0.25;
const MIN_SPLIT_SIZE: usize = 10;

#[derive(Debug, Default, serde::Serialize)]
pub struct ClusterReport {
    pub nodes: usize,
    pub clusters: usize,
}

#[derive(Debug, SurrealValue)]
struct EdgeRow {
    r#in: RecordId,
    out: RecordId,
    score: Option<f64>,
}
#[derive(Debug, SurrealValue)]
struct NodeRow {
    id: RecordId,
}

/// Undirected weighted graph as compact adjacency.
struct Graph {
    adj: Vec<Vec<(usize, f64)>>,
    k: Vec<f64>,
    two_m: f64,
}

fn louvain(adj: &[Vec<(usize, f64)>], k: &[f64], two_m: f64, members: &[usize]) -> Vec<usize> {
    // `members` are global node indices participating in this (sub)graph.
    let n = members.len();
    if n == 0 {
        return vec![];
    }
    let pos: HashMap<usize, usize> = members.iter().enumerate().map(|(i, &g)| (g, i)).collect();
    let mut comm: Vec<usize> = (0..n).collect();
    // community total degree
    let mut ctot: Vec<f64> = (0..n).map(|i| k[members[i]]).collect();

    let mut improved = true;
    let mut passes = 0;
    while improved && passes < 20 {
        improved = false;
        passes += 1;
        for i in 0..n {
            let gi = members[i];
            let ci = comm[i];
            // weights to neighbor communities (restricted to this subgraph)
            let mut w: HashMap<usize, f64> = HashMap::new();
            for &(gj, wt) in &adj[gi] {
                if let Some(&j) = pos.get(&gj) {
                    if j != i {
                        *w.entry(comm[j]).or_insert(0.0) += wt;
                    }
                }
            }
            // remove i from ci
            ctot[ci] -= k[gi];
            let ki = k[gi];
            // pick best community by modularity gain
            let mut best_c = ci;
            let mut best_gain = w.get(&ci).copied().unwrap_or(0.0) - ctot[ci] * ki / two_m;
            for (&c, &wic) in &w {
                let gain = wic - ctot[c] * ki / two_m;
                if gain > best_gain + 1e-12 || (gain > best_gain - 1e-12 && c < best_c) {
                    best_gain = gain;
                    best_c = c;
                }
            }
            ctot[best_c] += ki;
            if best_c != ci {
                comm[i] = best_c;
                improved = true;
            } else {
                comm[i] = ci;
            }
        }
    }
    comm
}

fn relabel(raw: &[usize]) -> Vec<usize> {
    let mut map = HashMap::new();
    let mut next = 0;
    raw.iter()
        .map(|&c| {
            *map.entry(c).or_insert_with(|| {
                let v = next;
                next += 1;
                v
            })
        })
        .collect()
}

pub async fn cluster(store: &Store) -> Result<ClusterReport> {
    let p = store.project.clone();

    let mut nr = store
        .db
        .query("SELECT id FROM atlas_node WHERE project=$p ORDER BY id")
        .bind(("p", p.clone()))
        .await?;
    let nodes: Vec<RecordId> = nr
        .take::<Vec<NodeRow>>(0)
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.id)
        .collect();
    let n = nodes.len();
    if n == 0 {
        return Ok(ClusterReport::default());
    }
    let idx: HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, r)| (rid_to_string(r), i))
        .collect();

    let mut er = store
        .db
        .query(
            "SELECT in, out, score FROM atlas_edge WHERE \
             in IN (SELECT VALUE id FROM atlas_node WHERE project=$p)",
        )
        .bind(("p", p.clone()))
        .await?;
    let edges: Vec<EdgeRow> = er.take(0).unwrap_or_default();

    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    let mut k = vec![0.0f64; n];
    let mut two_m = 0.0;
    for e in &edges {
        let (Some(&a), Some(&b)) = (
            idx.get(&rid_to_string(&e.r#in)),
            idx.get(&rid_to_string(&e.out)),
        ) else {
            continue;
        };
        if a == b {
            continue;
        }
        let w = e.score.unwrap_or(1.0).max(0.0001);
        adj[a].push((b, w));
        adj[b].push((a, w));
        k[a] += w;
        k[b] += w;
        two_m += 2.0 * w;
    }
    if two_m == 0.0 {
        // No edges → every node its own singleton cluster.
        for (i, nid) in nodes.iter().enumerate() {
            let _ = store
                .db
                .query("UPDATE $id SET cluster=$c")
                .bind(("id", nid.clone()))
                .bind(("c", i as i64))
                .await;
        }
        return Ok(ClusterReport {
            nodes: n,
            clusters: n,
        });
    }
    let g = Graph { adj, k, two_m };

    // Level-1 Louvain over all nodes.
    let all: Vec<usize> = (0..n).collect();
    let mut comm = relabel(&louvain(&g.adj, &g.k, g.two_m, &all));

    // Refinement: re-split any oversized cluster (>25% of N, ≥2·min).
    let mut next_id = comm.iter().copied().max().unwrap_or(0) + 1;
    let limit = ((n as f64) * MAX_CLUSTER_FRACTION).ceil() as usize;
    let mut by_c: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &c) in comm.iter().enumerate() {
        by_c.entry(c).or_default().push(i);
    }
    let mut to_split: Vec<Vec<usize>> = by_c
        .into_values()
        .filter(|m| m.len() > limit && m.len() >= 2 * MIN_SPLIT_SIZE)
        .collect();
    to_split.sort_by_key(|m| m.first().copied().unwrap_or(0));
    for members in to_split {
        let sub = louvain(&g.adj, &g.k, g.two_m, &members);
        // map sub-communities → fresh global ids, only if it actually splits
        let distinct: std::collections::HashSet<usize> = sub.iter().copied().collect();
        if distinct.len() < 2 {
            continue;
        }
        let mut remap: HashMap<usize, usize> = HashMap::new();
        for (mi, &gi) in members.iter().enumerate() {
            let sc = sub[mi];
            let nc = *remap.entry(sc).or_insert_with(|| {
                let v = next_id;
                next_id += 1;
                v
            });
            comm[gi] = nc;
        }
    }
    let comm = relabel(&comm);

    // Write cluster ids back (batched per cluster).
    let mut groups: HashMap<usize, Vec<RecordId>> = HashMap::new();
    for (i, &c) in comm.iter().enumerate() {
        groups.entry(c).or_default().push(nodes[i].clone());
    }
    let clusters = groups.len();
    for (c, ids) in groups {
        let _ = store
            .db
            .query("UPDATE atlas_node SET cluster=$c WHERE id IN $ids")
            .bind(("c", c as i64))
            .bind(("ids", ids))
            .await;
    }
    Ok(ClusterReport { nodes: n, clusters })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mk(store: &Store, n: &str) {
        store
            .db
            .query("UPSERT type::record($id) SET project='demo', kind='concept', label=$l, cluster=-1, created_at='2026-01-01'")
            .bind(("id", format!("atlas_node:{n}")))
            .bind(("l", n.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    async fn rel(store: &Store, x: &str, y: &str, w: f64) {
        store
            .db
            .query("RELATE $a->atlas_edge->$b SET relation='related', via='t', score=$w, created_at='2026-01-01'")
            .bind(("a", RecordId::new("atlas_node", x.to_string())))
            .bind(("b", RecordId::new("atlas_node", y.to_string())))
            .bind(("w", w))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn two_cliques_two_clusters() {
        let db = hifz_core::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        // Two triangles joined by one weak bridge → expect 2 clusters.
        for nname in ["a1", "a2", "a3", "b1", "b2", "b3"] {
            mk(&store, nname).await;
        }
        for (x, y) in [("a1", "a2"), ("a2", "a3"), ("a1", "a3")] {
            rel(&store, x, y, 5.0).await;
        }
        for (x, y) in [("b1", "b2"), ("b2", "b3"), ("b1", "b3")] {
            rel(&store, x, y, 5.0).await;
        }
        rel(&store, "a1", "b1", 0.1).await; // weak bridge

        let r = cluster(&store).await.unwrap();
        assert_eq!(r.nodes, 6);
        #[derive(Debug, SurrealValue)]
        struct Cl {
            cluster: Option<i64>,
            label: Option<String>,
        }
        let mut q = store
            .db
            .query("SELECT cluster, label FROM atlas_node WHERE project='demo'")
            .await
            .unwrap();
        let rows: Vec<Cl> = q.take(0).unwrap_or_default();
        let cof = |pre: &str| {
            rows.iter()
                .find(|r| {
                    r.label
                        .as_deref()
                        .map(|l| l.starts_with(pre))
                        .unwrap_or(false)
                })
                .and_then(|r| r.cluster)
        };
        // a* share a cluster, b* share a cluster, and the two differ.
        let a = rows
            .iter()
            .filter(|r| {
                r.label
                    .as_deref()
                    .map(|l| l.starts_with('a'))
                    .unwrap_or(false)
            })
            .filter_map(|r| r.cluster)
            .collect::<std::collections::HashSet<_>>();
        let b = rows
            .iter()
            .filter(|r| {
                r.label
                    .as_deref()
                    .map(|l| l.starts_with('b'))
                    .unwrap_or(false)
            })
            .filter_map(|r| r.cluster)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(a.len(), 1, "a-clique cohesive");
        assert_eq!(b.len(), 1, "b-clique cohesive");
        assert_ne!(cof("a"), cof("b"), "two communities separated");
    }
}
