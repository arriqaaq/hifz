//! **Canonical Leiden** (Traag, Waltman, van Eck, *Sci Rep* 9:5233, 2019 —
//! arXiv:1810.08473): local-moving → seeded randomized refinement →
//! aggregation, recursing until convergence. Writes `cluster` onto
//! `atlas_node`.
//!
//! - **Connected-community guarantee:** refinement only ever merges a node
//!   that is still its own singleton, and only into a sub-community it shares
//!   an edge with — so every output community's induced subgraph is connected
//!   (the property single-level local-moving / Louvain does *not* give;
//!   Louvain leaves ≤25% badly-connected / ≤16% disconnected communities per
//!   the paper). The γ well-connectedness threshold governs *separation*,
//!   not connectivity; we use modularity (RB, γ=`RESOLUTION`=1) as the
//!   quality function — so this is "Leiden with the connectivity guarantee",
//!   not a claim of provable γ-optimality.
//! - **Deterministic:** all randomness flows from one fixed-seed PRNG
//!   (`Rng`, seeded from `SEED`); a seeded PRNG is the textbook-correct way
//!   to make Leiden reproducible (Traag's reference is randomized but
//!   reproducible under a fixed seed) — not a hack. No `std::HashMap` is on
//!   any decision/accumulation path (its iteration order is process-random);
//!   community accumulators are `BTreeMap`, sums are in sorted-index order,
//!   every argmax breaks ties by index.
//! - **No external dependency** (no `rand`, no Leiden crate — the surveyed
//!   crates were either non-seedable/non-reproducible or alpha).

use std::collections::{BTreeMap, VecDeque};

use anyhow::Result;
use kernel::ids::rid_to_string;
use surrealdb::types::{RecordId, SurrealValue};

use crate::store::Store;

/// Modularity resolution γ (1.0 = classic modularity; matches the prior
/// single-level local-moving semantics so the planted-partition test holds).
const RESOLUTION: f64 = 1.0;
/// Refinement randomness temperature θ (Traag default ≈0.05).
const THETA: f64 = 0.05;
/// Fixed PRNG seed → reproducible partitions across runs/processes.
const SEED: u64 = 0x5EED_A71A5_u64;
/// Safety bound on aggregation levels (Leiden converges in a handful).
const MAX_LEVELS: usize = 64;
/// Modularity-gain comparison epsilon.
const EPS: f64 = 1e-12;

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

// --- deterministic PRNG (splitmix64 seed → xoshiro256**) -----------------

struct Rng {
    s: [u64; 4],
}
impl Rng {
    fn seeded(seed: u64) -> Self {
        let mut sm = seed;
        let mut next = || {
            sm = sm.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = sm;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Rng {
            s: [next(), next(), next(), next()],
        }
    }
    fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }
    fn next_f64(&mut self) -> f64 {
        // 53-bit mantissa in [0, 1)
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Deterministic Fisher–Yates.
    fn shuffle(&mut self, v: &mut [usize]) {
        for i in (1..v.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

// --- graph ---------------------------------------------------------------

/// Undirected weighted graph. `adj[i]` is coalesced (one entry per neighbour,
/// weight summed) and **sorted by neighbour index** for deterministic
/// iteration. `k[i]` is the weighted degree *including* `2·self_loop[i]`
/// (a self-loop of weight s adds 2s to degree). `two_m == Σ k[i]`.
struct Graph {
    n: usize,
    adj: Vec<Vec<(usize, f64)>>,
    k: Vec<f64>,
    self_loop: Vec<f64>,
    two_m: f64,
}

/// First-occurrence relabel to a contiguous `0..C` range (deterministic:
/// iterates the slice in index order, the map is only a seen-set).
fn relabel(raw: &[usize]) -> Vec<usize> {
    let mut map: BTreeMap<usize, usize> = BTreeMap::new();
    let mut next = 0usize;
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

/// Modularity Q of `comm` on `g` (γ=`RESOLUTION`). Self-loops enter via
/// `A_ii = 2·self_loop`. `g.adj` stores each undirected edge symmetrically,
/// so the inner sum counts an internal edge twice (= 2w), matching Σ A_ij.
/// Used by the oracle/property tests (not on the clustering hot path).
#[allow(dead_code)]
fn modularity(g: &Graph, comm: &[usize]) -> f64 {
    if g.two_m == 0.0 {
        return 0.0;
    }
    let c = comm.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut tot = vec![0.0f64; c];
    let mut inn = vec![0.0f64; c];
    for i in 0..g.n {
        tot[comm[i]] += g.k[i];
        inn[comm[i]] += 2.0 * g.self_loop[i];
    }
    for i in 0..g.n {
        for &(j, w) in &g.adj[i] {
            if comm[j] == comm[i] {
                inn[comm[i]] += w;
            }
        }
    }
    let two_m = g.two_m;
    (0..c)
        .map(|x| inn[x] / two_m - RESOLUTION * (tot[x] / two_m).powi(2))
        .sum()
}

// --- local moving (fast) -------------------------------------------------

/// Move nodes to the neighbouring community of maximum modularity gain,
/// queue-driven, until no positive move remains. Starts from whatever
/// `part` is (singletons at level 0, induced partition at aggregate levels).
/// `part` is relabelled contiguous on entry.
fn move_nodes_fast(g: &Graph, part: &mut Vec<usize>, rng: &mut Rng) {
    *part = relabel(part);
    let two_m = g.two_m;
    if two_m == 0.0 {
        return;
    }
    let c = part.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut tot = vec![0.0f64; c];
    for i in 0..g.n {
        tot[part[i]] += g.k[i];
    }
    let mut order: Vec<usize> = (0..g.n).collect();
    rng.shuffle(&mut order);
    let mut queue: VecDeque<usize> = order.into_iter().collect();
    let mut in_q = vec![true; g.n];

    while let Some(i) = queue.pop_front() {
        in_q[i] = false;
        let ci = part[i];
        let ki = g.k[i];
        // Edge weight from i to each neighbour community (BTreeMap → the
        // candidate scan below is in sorted community order = deterministic).
        let mut w: BTreeMap<usize, f64> = BTreeMap::new();
        for &(j, wt) in &g.adj[i] {
            if j != i {
                *w.entry(part[j]).or_insert(0.0) += wt;
            }
        }
        tot[ci] -= ki; // remove i from its community
        let mut best_c = ci;
        let mut best_gain = w.get(&ci).copied().unwrap_or(0.0) - RESOLUTION * tot[ci] * ki / two_m;
        for (&cc, &wic) in &w {
            let gain = wic - RESOLUTION * tot[cc] * ki / two_m;
            if gain > best_gain + EPS || (gain > best_gain - EPS && cc < best_c) {
                best_gain = gain;
                best_c = cc;
            }
        }
        tot[best_c] += ki;
        if best_c != ci {
            part[i] = best_c;
            for &(j, _) in &g.adj[i] {
                if part[j] != best_c && !in_q[j] {
                    in_q[j] = true;
                    queue.push_back(j);
                }
            }
        }
    }
}

// --- refinement ----------------------------------------------------------

/// Within each coarse community, build **connected** sub-communities: only a
/// still-singleton node may merge, and only into a sub-community it shares an
/// edge with (→ connectivity by induction), chosen with prob ∝ exp(ΔQ/θ)
/// among non-negative-gain candidates. Returns a refined partition (each
/// refined community ⊆ one coarse community).
fn refine(g: &Graph, part: &[usize], rng: &mut Rng) -> Vec<usize> {
    let two_m = g.two_m;
    let mut refined: Vec<usize> = (0..g.n).collect();
    if two_m == 0.0 {
        return refined;
    }
    // Coarse community → its member node indices (BTreeMap → deterministic
    // community order; members pushed in node-index order).
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..g.n {
        groups.entry(part[i]).or_default().push(i);
    }
    // tot/size per refined community id; id == a node index (singleton seed).
    let mut tot: Vec<f64> = g.k.clone();
    let mut size: Vec<usize> = vec![1; g.n];

    for (_s, members) in &groups {
        let mut order = members.clone();
        rng.shuffle(&mut order);
        for _pass in 0..16 {
            let mut moved = false;
            for &v in &order {
                // Only a node still in its own singleton may be merged.
                if refined[v] != v || size[v] != 1 {
                    continue;
                }
                let cv = part[v];
                let kv = g.k[v];
                // Weight from v to each refined sub-community *within S*.
                let mut w: BTreeMap<usize, f64> = BTreeMap::new();
                for &(j, wt) in &g.adj[v] {
                    if j != v && part[j] == cv {
                        *w.entry(refined[j]).or_insert(0.0) += wt;
                    }
                }
                if w.is_empty() {
                    continue; // no edge inside S → stays a singleton (connected trivially)
                }
                // Candidates with ΔQ ≥ 0 (v currently isolated: tot of its
                // own singleton is kv, removed → 0). Sorted by community id.
                let mut cands: Vec<(usize, f64)> = Vec::new();
                let mut max_g = f64::NEG_INFINITY;
                for (&t, &wvt) in &w {
                    let gain = wvt - RESOLUTION * tot[t] * kv / two_m;
                    if gain >= -EPS {
                        if gain > max_g {
                            max_g = gain;
                        }
                        cands.push((t, gain));
                    }
                }
                if cands.is_empty() {
                    continue;
                }
                // Sample T ∝ exp((gain - max)/θ) (shift for stability).
                let mut wsum = 0.0;
                let probs: Vec<f64> = cands
                    .iter()
                    .map(|&(_, gigi)| {
                        let p = ((gigi - max_g) / THETA).exp();
                        wsum += p;
                        p
                    })
                    .collect();
                let mut u = rng.next_f64() * wsum;
                let mut chosen = cands[cands.len() - 1].0;
                for (idx, &(t, _)) in cands.iter().enumerate() {
                    u -= probs[idx];
                    if u <= 0.0 {
                        chosen = t;
                        break;
                    }
                }
                // Move v's singleton into `chosen`.
                tot[v] -= kv;
                tot[chosen] += kv;
                size[v] -= 1;
                size[chosen] += 1;
                refined[v] = chosen;
                moved = true;
            }
            if !moved {
                break;
            }
        }
    }
    refined
}

// --- aggregation ---------------------------------------------------------

/// Collapse `refined` (relabelled 0..R) into super-nodes. Returns the
/// aggregate graph, the lifted coarse partition (super-node → coarse
/// community of its members), and the old→super index map for flattening.
fn aggregate(g: &Graph, refined: &[usize], coarse: &[usize]) -> (Graph, Vec<usize>, Vec<usize>) {
    let r = refined.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut adj_m: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); r];
    let mut self_loop = vec![0.0f64; r];
    let mut k = vec![0.0f64; r];
    let mut lifted = vec![usize::MAX; r];

    for i in 0..g.n {
        let ri = refined[i];
        k[ri] += g.k[i];
        self_loop[ri] += g.self_loop[i];
        if lifted[ri] == usize::MAX {
            lifted[ri] = coarse[i];
        } else {
            debug_assert_eq!(
                lifted[ri], coarse[i],
                "refine must not span coarse communities"
            );
        }
        for &(j, w) in &g.adj[i] {
            let rj = refined[j];
            if ri == rj {
                // internal edge seen at i and at j → 0.5·w each ⇒ total w
                self_loop[ri] += 0.5 * w;
            } else {
                *adj_m[ri].entry(rj).or_insert(0.0) += w;
            }
        }
    }
    let adj: Vec<Vec<(usize, f64)>> = adj_m.into_iter().map(|m| m.into_iter().collect()).collect();
    let two_m: f64 = {
        // sum in index order (deterministic float accumulation)
        let mut s = 0.0;
        for x in &k {
            s += *x;
        }
        s
    };
    (
        Graph {
            n: r,
            adj,
            k,
            self_loop,
            two_m,
        },
        lifted,
        refined.to_vec(),
    )
}

// --- driver --------------------------------------------------------------

/// Full Leiden. Returns a contiguous community id per original node.
fn leiden(mut g: Graph, rng: &mut Rng) -> Vec<usize> {
    let n0 = g.n;
    if n0 == 0 {
        return Vec::new();
    }
    let mut part: Vec<usize> = (0..n0).collect(); // singletons
    let mut maps: Vec<Vec<usize>> = Vec::new();

    for _ in 0..MAX_LEVELS {
        move_nodes_fast(&g, &mut part, rng);
        part = relabel(&part);
        let ncomm = part.iter().copied().max().map(|m| m + 1).unwrap_or(0);
        if ncomm >= g.n {
            break; // every node its own community → converged
        }
        let refined = relabel(&refine(&g, &part, rng));
        let nref = refined.iter().copied().max().map(|m| m + 1).unwrap_or(0);
        if nref >= g.n {
            break; // refinement could not coarsen → done
        }
        let (g2, part2, map) = aggregate(&g, &refined, &part);
        maps.push(map);
        g = g2;
        part = part2;
    }

    // Flatten original node → final community through the aggregation maps.
    let mut out = vec![0usize; n0];
    for (o, slot) in out.iter_mut().enumerate() {
        let mut idx = o;
        for m in &maps {
            idx = m[idx];
        }
        *slot = part[idx];
    }
    relabel(&out)
}

// --- public entry --------------------------------------------------------

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
    // id string → index (lookup only; not an iteration-order dependency).
    let idx: BTreeMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, r)| (rid_to_string(r), i))
        .collect();

    let mut er = store
        .db
        .query("SELECT in, out, score FROM atlas_edge WHERE project=$p")
        .bind(("p", p.clone()))
        .await?;
    let edges: Vec<EdgeRow> = er.take(0).unwrap_or_default();

    // Coalesce parallel edges (sum) and sort each adjacency by neighbour
    // index → iteration order is independent of DB row order.
    let mut adj_m: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); n];
    let mut self_loop = vec![0.0f64; n];
    let mut k = vec![0.0f64; n];
    let mut two_m = 0.0;
    for e in &edges {
        let (Some(&a), Some(&b)) = (
            idx.get(&rid_to_string(&e.r#in)),
            idx.get(&rid_to_string(&e.out)),
        ) else {
            continue;
        };
        let w = e.score.unwrap_or(1.0).max(0.0001);
        if a == b {
            self_loop[a] += w;
            k[a] += 2.0 * w;
            two_m += 2.0 * w;
            continue;
        }
        *adj_m[a].entry(b).or_insert(0.0) += w;
        *adj_m[b].entry(a).or_insert(0.0) += w;
        k[a] += w;
        k[b] += w;
        two_m += 2.0 * w;
    }

    if two_m == 0.0 {
        // No edges → every node its own singleton cluster (preserved
        // behaviour + fallback).
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

    let adj: Vec<Vec<(usize, f64)>> = adj_m.into_iter().map(|m| m.into_iter().collect()).collect();
    let g = Graph {
        n,
        adj,
        k,
        self_loop,
        two_m,
    };

    let mut rng = Rng::seeded(SEED);
    let comm = leiden(g, &mut rng);

    // Write cluster ids back (BTreeMap → deterministic UPDATE order).
    let mut groups: BTreeMap<usize, Vec<RecordId>> = BTreeMap::new();
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

    // ---- in-memory graph builder + invariants (oracle helpers) ----------

    /// Build a `Graph` from an undirected weighted edge list.
    fn graph(n: usize, edges: &[(usize, usize, f64)]) -> Graph {
        let mut adj_m: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); n];
        let mut k = vec![0.0; n];
        let mut self_loop = vec![0.0; n];
        let mut two_m = 0.0;
        for &(a, b, w) in edges {
            if a == b {
                self_loop[a] += w;
                k[a] += 2.0 * w;
                two_m += 2.0 * w;
            } else {
                *adj_m[a].entry(b).or_insert(0.0) += w;
                *adj_m[b].entry(a).or_insert(0.0) += w;
                k[a] += w;
                k[b] += w;
                two_m += 2.0 * w;
            }
        }
        Graph {
            n,
            adj: adj_m.into_iter().map(|m| m.into_iter().collect()).collect(),
            k,
            self_loop,
            two_m,
        }
    }

    /// Every community's induced subgraph must be connected (the Leiden
    /// guarantee). Returns true if the partition satisfies it.
    fn all_communities_connected(g: &Graph, comm: &[usize]) -> bool {
        use std::collections::BTreeSet;
        let mut by_c: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, &c) in comm.iter().enumerate() {
            by_c.entry(c).or_default().push(i);
        }
        for members in by_c.values() {
            let set: BTreeSet<usize> = members.iter().copied().collect();
            let start = members[0];
            let mut seen: BTreeSet<usize> = BTreeSet::new();
            let mut stack = vec![start];
            seen.insert(start);
            while let Some(u) = stack.pop() {
                for &(v, _) in &g.adj[u] {
                    if set.contains(&v) && seen.insert(v) {
                        stack.push(v);
                    }
                }
            }
            if seen.len() != members.len() {
                return false; // disconnected community
            }
        }
        true
    }

    fn run(g: Graph) -> (Vec<usize>, Graph) {
        // leiden consumes g; rebuild an identical one for invariant checks.
        let clone = Graph {
            n: g.n,
            adj: g.adj.clone(),
            k: g.k.clone(),
            self_loop: g.self_loop.clone(),
            two_m: g.two_m,
        };
        let mut rng = Rng::seeded(SEED);
        (leiden(g, &mut rng), clone)
    }

    // ---- T0: modularity helper, hand-computed oracle --------------------

    #[test]
    fn t0_modularity_hand_computed() {
        // Two triangles (w=5) + a weak bridge a1-b1 (w=0.1).
        // Hand-derived Q for the 2-community split ≈ 0.49668 (see plan).
        let g = graph(
            6,
            &[
                (0, 1, 5.0),
                (1, 2, 5.0),
                (0, 2, 5.0),
                (3, 4, 5.0),
                (4, 5, 5.0),
                (3, 5, 5.0),
                (0, 3, 0.1),
            ],
        );
        let q = modularity(&g, &[0, 0, 0, 1, 1, 1]);
        assert!(
            (q - 0.496_68).abs() < 1e-3,
            "Q(2-clique split) ≈ 0.4967, got {q}"
        );
        // Singletons must score worse than the good split.
        let q_single = modularity(&g, &[0, 1, 2, 3, 4, 5]);
        assert!(q_single < q, "singletons {q_single} < split {q}");
    }

    // ---- T1: Zachary karate club (cross-tool reference oracle) ----------

    /// Canonical Zachary karate club, 0-indexed, 78 undirected edges
    /// (identical to networkx `karate_club_graph`).
    const KARATE: &[(usize, usize)] = &[
        (0, 1),
        (0, 2),
        (0, 3),
        (0, 4),
        (0, 5),
        (0, 6),
        (0, 7),
        (0, 8),
        (0, 10),
        (0, 11),
        (0, 12),
        (0, 13),
        (0, 17),
        (0, 19),
        (0, 21),
        (0, 31),
        (1, 2),
        (1, 3),
        (1, 7),
        (1, 13),
        (1, 17),
        (1, 19),
        (1, 21),
        (1, 30),
        (2, 3),
        (2, 7),
        (2, 8),
        (2, 9),
        (2, 13),
        (2, 27),
        (2, 28),
        (2, 32),
        (3, 7),
        (3, 12),
        (3, 13),
        (4, 6),
        (4, 10),
        (5, 6),
        (5, 10),
        (5, 16),
        (6, 16),
        (8, 30),
        (8, 32),
        (8, 33),
        (9, 33),
        (13, 33),
        (14, 32),
        (14, 33),
        (15, 32),
        (15, 33),
        (18, 32),
        (18, 33),
        (19, 33),
        (20, 32),
        (20, 33),
        (22, 32),
        (22, 33),
        (23, 25),
        (23, 27),
        (23, 29),
        (23, 32),
        (23, 33),
        (24, 25),
        (24, 27),
        (24, 31),
        (25, 31),
        (26, 29),
        (26, 33),
        (27, 33),
        (28, 31),
        (28, 33),
        (29, 32),
        (29, 33),
        (30, 32),
        (30, 33),
        (31, 32),
        (31, 33),
        (32, 33),
    ];

    #[test]
    fn t1_karate_matches_reference_properties() {
        let edges: Vec<(usize, usize, f64)> = KARATE.iter().map(|&(a, b)| (a, b, 1.0)).collect();
        let (comm, g) = run(graph(34, &edges));
        let nc = comm.iter().copied().max().unwrap() + 1;
        // networkx/igraph/leidenalg all land in [2,4] on karate.
        assert!((2..=4).contains(&nc), "communities in [2,4], got {nc}");
        // Reference optimum ≈ 0.4198; every reference impl clears ≥0.41.
        let q = modularity(&g, &comm);
        assert!(q >= 0.40, "modularity ≥ 0.40 (ref ≈0.4198), got {q}");
        // Ground-truth faction split: instructor 0 vs president 33.
        assert_ne!(comm[0], comm[33], "node 0 and 33 in different communities");
        // The Leiden guarantee.
        assert!(
            all_communities_connected(&g, &comm),
            "all communities connected"
        );
    }

    // ---- T2: planted partitions ----------------------------------------

    #[test]
    fn t2_two_cliques_and_three_clique_ring() {
        // 2 triangles + weak bridge → exactly 2 cohesive communities.
        let (c2, g2) = run(graph(
            6,
            &[
                (0, 1, 5.0),
                (1, 2, 5.0),
                (0, 2, 5.0),
                (3, 4, 5.0),
                (4, 5, 5.0),
                (3, 5, 5.0),
                (0, 3, 0.1),
            ],
        ));
        assert_eq!(c2[0], c2[1], "clique A cohesive");
        assert_eq!(c2[1], c2[2], "clique A cohesive");
        assert_eq!(c2[3], c2[4], "clique B cohesive");
        assert_ne!(c2[0], c2[3], "two communities separated");
        assert!(all_communities_connected(&g2, &c2));

        // 3 cliques in a ring joined by weak bridges → 3 communities.
        let (c3, g3) = run(graph(
            9,
            &[
                (0, 1, 5.0),
                (1, 2, 5.0),
                (0, 2, 5.0),
                (3, 4, 5.0),
                (4, 5, 5.0),
                (3, 5, 5.0),
                (6, 7, 5.0),
                (7, 8, 5.0),
                (6, 8, 5.0),
                (2, 3, 0.1),
                (5, 6, 0.1),
                (8, 0, 0.1),
            ],
        ));
        assert_eq!(c3.iter().copied().max().unwrap() + 1, 3, "3 cliques → 3");
        assert!(all_communities_connected(&g3, &c3));
    }

    // ---- T3: connectivity invariant on a Louvain failure pattern -------

    #[test]
    fn t3_no_disconnected_community() {
        // Two dense cliques with NO edge between them, each bridged only
        // through a shared sparse hub — a shape where naive single-level
        // local-moving can group the two cliques into one (disconnected)
        // community. Leiden's refinement must prevent that.
        let mut e: Vec<(usize, usize, f64)> = Vec::new();
        for a in 0..5 {
            for b in (a + 1)..5 {
                e.push((a, b, 10.0)); // clique A 0..4
            }
        }
        for a in 5..10 {
            for b in (a + 1)..10 {
                e.push((a, b, 10.0)); // clique B 5..9
            }
        }
        e.push((0, 10, 0.2)); // hub 10 weakly bridges both, A and B not adjacent
        e.push((5, 10, 0.2));
        let (comm, g) = run(graph(11, &e));
        assert!(
            all_communities_connected(&g, &comm),
            "no community may be internally disconnected"
        );
    }

    // ---- T4: determinism -----------------------------------------------

    #[test]
    fn t4_deterministic() {
        let edges: Vec<(usize, usize, f64)> = KARATE.iter().map(|&(a, b)| (a, b, 1.0)).collect();
        let a = run(graph(34, &edges)).0;
        let b = run(graph(34, &edges)).0;
        assert_eq!(a, b, "same seed → byte-identical partition");
    }

    #[tokio::test]
    async fn t4_cluster_deterministic_on_db() {
        async fn build() -> Vec<i64> {
            let db = kernel::db::connect_mem().await.unwrap();
            crate::store::init_atlas_schema(&db, 384).await.unwrap();
            let store = Store::new(db, "demo");
            for nm in ["a1", "a2", "a3", "b1", "b2", "b3"] {
                store
                    .db
                    .query("UPSERT type::record($id) SET project='demo', kind='concept', label=$l, cluster=-1, created_at='2026-01-01'")
                    .bind(("id", format!("atlas_node:{nm}")))
                    .bind(("l", nm.to_string()))
                    .await
                    .unwrap()
                    .check()
                    .unwrap();
            }
            for (x, y, w) in [
                ("a1", "a2", 5.0),
                ("a2", "a3", 5.0),
                ("a1", "a3", 5.0),
                ("b1", "b2", 5.0),
                ("b2", "b3", 5.0),
                ("b1", "b3", 5.0),
                ("a1", "b1", 0.1),
            ] {
                store
                    .db
                    .query("RELATE $a->atlas_edge->$b SET project='demo', relation='related', via='t', score=$w, created_at='2026-01-01'")
                    .bind(("a", RecordId::new("atlas_node", x.to_string())))
                    .bind(("b", RecordId::new("atlas_node", y.to_string())))
                    .bind(("w", w))
                    .await
                    .unwrap();
            }
            cluster(&store).await.unwrap();
            #[derive(Debug, SurrealValue)]
            struct C {
                cluster: Option<i64>,
            }
            let mut q = store
                .db
                .query("SELECT id, cluster FROM atlas_node WHERE project='demo' ORDER BY id")
                .await
                .unwrap();
            q.take::<Vec<C>>(0)
                .unwrap()
                .into_iter()
                .map(|c| c.cluster.unwrap_or(-1))
                .collect()
        }
        assert_eq!(build().await, build().await, "DB clustering reproducible");
    }

    // ---- T2/T5: edge cases + the existing planted test (kept) ----------

    #[tokio::test]
    async fn two_cliques_two_clusters() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        for nname in ["a1", "a2", "a3", "b1", "b2", "b3"] {
            store
                .db
                .query("UPSERT type::record($id) SET project='demo', kind='concept', label=$l, cluster=-1, created_at='2026-01-01'")
                .bind(("id", format!("atlas_node:{nname}")))
                .bind(("l", nname.to_string()))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
        for (x, y) in [("a1", "a2"), ("a2", "a3"), ("a1", "a3")] {
            rel(&store, x, y, 5.0).await;
        }
        for (x, y) in [("b1", "b2"), ("b2", "b3"), ("b1", "b3")] {
            rel(&store, x, y, 5.0).await;
        }
        rel(&store, "a1", "b1", 0.1).await;

        let r = cluster(&store).await.unwrap();
        assert_eq!(r.nodes, 6);
        assert_eq!(r.clusters, 2, "two cliques → two clusters");
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
        let cof = |pre: char| {
            rows.iter()
                .filter(|r| {
                    r.label
                        .as_deref()
                        .map(|l| l.starts_with(pre))
                        .unwrap_or(false)
                })
                .filter_map(|r| r.cluster)
                .collect::<std::collections::HashSet<_>>()
        };
        assert_eq!(cof('a').len(), 1, "a-clique cohesive");
        assert_eq!(cof('b').len(), 1, "b-clique cohesive");
        assert_ne!(cof('a'), cof('b'), "communities separated");
    }

    async fn rel(store: &Store, x: &str, y: &str, w: f64) {
        store
            .db
            .query("RELATE $a->atlas_edge->$b SET project='demo', relation='related', via='t', score=$w, created_at='2026-01-01'")
            .bind(("a", RecordId::new("atlas_node", x.to_string())))
            .bind(("b", RecordId::new("atlas_node", y.to_string())))
            .bind(("w", w))
            .await
            .unwrap();
    }

    #[test]
    fn t5_edge_cases() {
        // Single node.
        assert_eq!(run(graph(1, &[])).0, vec![0]);
        // K5 complete → one community.
        let mut k5 = Vec::new();
        for a in 0..5 {
            for b in (a + 1)..5 {
                k5.push((a, b, 1.0));
            }
        }
        let (c, _) = run(graph(5, &k5));
        assert_eq!(c.iter().copied().max().unwrap() + 1, 1, "K5 → 1 community");
        // Two disconnected triangles → ≥2, each connected.
        let (c2, g2) = run(graph(
            6,
            &[
                (0, 1, 1.0),
                (1, 2, 1.0),
                (0, 2, 1.0),
                (3, 4, 1.0),
                (4, 5, 1.0),
                (3, 5, 1.0),
            ],
        ));
        assert!(c2.iter().copied().max().unwrap() + 1 >= 2);
        assert!(all_communities_connected(&g2, &c2));
    }

    // ---- T6: property/fuzz — universal invariants over random graphs ----

    #[test]
    fn t6_property_invariants_over_random_graphs() {
        let mut seed = Rng::seeded(0xF0F0_1234);
        for _trial in 0..50 {
            let n = 5 + (seed.next_u64() % 60) as usize;
            let mut edges = Vec::new();
            // Erdős–Rényi-ish + a guaranteed-disconnected case sometimes.
            let p_num = 1 + (seed.next_u64() % 6); // density knob
            for a in 0..n {
                for b in (a + 1)..n {
                    if seed.next_u64() % 20 < p_num {
                        let w = 1.0 + (seed.next_u64() % 5) as f64;
                        edges.push((a, b, w));
                    }
                }
            }
            let g = graph(n, &edges);
            if g.two_m == 0.0 {
                continue; // no-edge graphs handled by the cluster() fallback
            }
            let (comm, gg) = run(graph(n, &edges));

            // (i) connectivity guarantee.
            assert!(
                all_communities_connected(&gg, &comm),
                "trial graph produced a disconnected community"
            );
            // (ii) never worse than the trivial partitions.
            let q = modularity(&gg, &comm);
            let singletons: Vec<usize> = (0..n).collect();
            let one = vec![0usize; n];
            let q_floor = modularity(&gg, &singletons).max(modularity(&gg, &one));
            assert!(
                q >= q_floor - 1e-9,
                "Q {q} must be ≥ trivial floor {q_floor}"
            );
            // (iii) determinism on repeat.
            let again = run(graph(n, &edges)).0;
            assert_eq!(comm, again, "non-deterministic on a random graph");
        }
    }
}
