//! Benchmark: project-scoped maktab_edge query A/B.
//!
//! Why this exists separately from the unit suite: an at-scale perf check
//! seeds tens of thousands of awaited inserts into an in-mem SurrealKV.
//! That's minutes, not milliseconds — unit tests should not be benchmarks.
//! Run only on demand:
//!
//!     cargo run --release --features maktab --bin maktab-edge-scaling-bench
//!
//! Compares two query shapes on the SAME fixture:
//!   A. `WHERE project=$p`              (the FIX — indexed via maktab_edge_project)
//!   B. `WHERE in IN (SELECT VALUE id FROM maktab_node WHERE project=$p)`
//!      (the OLD shape — hung >18s on real data even with FIELDS in indexes;
//!      kept here purely as the discriminating control. The anti-pattern
//!      source-grep test in crates/maktab/src/store.rs forbids it in
//!      production code.)
//!
//! Defaults are sized so A is sub-second and B is visibly slower; bump
//! with `--p-edges N --n-edges N` if you want to push harder.

use std::time::Instant;

use maktab::init_maktab_schema;
use surrealdb::types::{RecordId, SurrealValue};

#[derive(Debug, SurrealValue)]
struct E {
    r#in: RecordId,
    #[allow(dead_code)]
    out: RecordId,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Tiny arg parser (no clap dep): --p-edges N, --n-edges N
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str, default: usize| -> usize {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(default)
    };
    let p_nodes = arg("--p-nodes", 200);
    let p_edges = arg("--p-edges", 1_000);
    let n_nodes = arg("--n-nodes", 5_000);
    let n_edges = arg("--n-edges", 10_000);

    let db = kernel::db::connect_mem().await?;
    init_maktab_schema(&db, 384).await?;

    eprintln!(
        "seeding: project p={p_nodes}n/{p_edges}e, noise n={n_nodes}n/{n_edges}e \
         (~{} awaited inserts) — this is the slow part",
        p_nodes + p_edges + n_nodes + n_edges
    );
    let t0 = Instant::now();
    for i in 0..p_nodes {
        db.query(
            "CREATE type::record($id) SET project='p', kind='concept', \
             label='x', cluster=-1, created_at='2026-01-01'",
        )
        .bind(("id", format!("maktab_node:p{i}")))
        .await?
        .check()?;
    }
    for i in 0..p_edges {
        db.query(
            "RELATE $a->maktab_edge->$b SET project='p', relation='related', \
             via='t', score=1.0, created_at='2026-01-01'",
        )
        .bind((
            "a",
            RecordId::new("maktab_node", format!("p{}", i % p_nodes)),
        ))
        .bind((
            "b",
            RecordId::new("maktab_node", format!("p{}", (i + 1) % p_nodes)),
        ))
        .await?;
    }
    for i in 0..n_nodes {
        db.query(
            "CREATE type::record($id) SET project='n', kind='concept', \
             label='x', cluster=-1, created_at='2026-01-01'",
        )
        .bind(("id", format!("maktab_node:n{i}")))
        .await?
        .check()?;
    }
    for i in 0..n_edges {
        db.query(
            "RELATE $a->maktab_edge->$b SET project='n', relation='related', \
             via='t', score=1.0, created_at='2026-01-01'",
        )
        .bind((
            "a",
            RecordId::new("maktab_node", format!("n{}", i % n_nodes)),
        ))
        .bind((
            "b",
            RecordId::new("maktab_node", format!("n{}", (i + 1) % n_nodes)),
        ))
        .await?;
    }
    eprintln!("seeded in {:?}", t0.elapsed());

    // (A) The fix shape — indexed via maktab_edge_project.
    let t = Instant::now();
    let a: Vec<E> = db
        .query("SELECT in, out FROM maktab_edge WHERE project='p'")
        .await?
        .take(0)
        .unwrap_or_default();
    let dt_a = t.elapsed();
    println!(
        "A. WHERE project='p'              -> {} rows in {dt_a:?}",
        a.len()
    );
    assert_eq!(a.len(), p_edges, "fix shape isolated to project p");

    // (B) The old (broken) shape — flat IN (subquery). Kept here as the
    // discriminating control; forbidden in production code by the
    // store.rs anti-pattern test.
    let t = Instant::now();
    let b: Vec<E> = db
        .query(
            "SELECT in, out FROM maktab_edge WHERE \
             in IN (SELECT VALUE id FROM maktab_node WHERE project='p')",
        )
        .await?
        .take(0)
        .unwrap_or_default();
    let dt_b = t.elapsed();
    println!(
        "B. WHERE in IN (SELECT VALUE …)   -> {} rows in {dt_b:?}",
        b.len()
    );

    let ratio = dt_b.as_secs_f64() / dt_a.as_secs_f64().max(1e-9);
    println!(
        "\nresult: A is {ratio:.1}x faster than B at p={p_edges}e, n={n_edges}e edges \
         (the fix). If A is slower or comparable to B, the index isn't being used."
    );
    Ok(())
}
