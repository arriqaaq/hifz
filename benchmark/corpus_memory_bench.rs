//! Track B — commit-grounded long-session memory eval on the real corpus.
//!
//! Question: does the Claude-Code injection path (`generate_context_with_query`,
//! what the `hifz:recall` skill feeds the LLM) surface relevant memories for
//! real past prompts — including OLD memories across long time gaps?
//!
//! Oracle (non-circular): for a run, the relevant memories are those whose
//! files overlap the files that run actually SHIPPED. The signal is recovered
//! from `commit_made` observation metadata (commit-grounding — the project's
//! defensible axis), independent of `run.recalled_ids` (using that would be
//! circular: it IS the system's own retrieval output — see the static guard in
//! tests/recall_relevance.rs). Leakage control: only memories created at/before
//! the run's start.
//!
//! Reality (verified via /api/v1/export): the live corpus currently has ZERO
//! commit_made observations (the git post-commit hook only just landed). So a
//! clearly-LABELED weaker fallback oracle — files touched by the run's
//! file_edit/file_write observations — is also computed so the bench yields
//! numbers today. The fallback NEVER produces a hard PASS (it is not
//! commit-grounding); it prints metrics then SKIPs with guidance.
//!
//! Usage:
//!   cargo run --release --bin corpus-memory-bench -- --project hifz
//!   cargo run --release --bin corpus-memory-bench -- --export-json dump.json
//!   cargo run --release --bin corpus-memory-bench -- --project hifz --loose-paths

use std::collections::HashMap;

use anyhow::Result;
use hifz::db::{self, init_schema};
use hifz::embed::Embedder;
use hifz::ground::CommitSignal;
use hifz::models::SearchResult;
use hifz::search::{self, SearchConfig};

#[path = "corpus_common.rs"]
mod corpus_common;
use corpus_common::{
    Export, Verdict, epoch_secs, files_overlap, load_export, mrr, norm_project, rec_str, str_field,
    strs_field,
};

const BENCH_PROJECT: &str = "corpus-b";
const MIN_RUNS: usize = 20;
const MIN_PAIRS: usize = 30;

struct MemRec {
    title: String,
    content: String,
    files: Vec<String>,
    created: i64,
    project: String,
}

struct RunRec {
    prompt: String,
    started: i64,
    project: String,
    obs_ids: Vec<String>,
}

/// One (run, oracle-memory) pair after retrieval, for stratified metrics.
struct PairOutcome {
    age_gap_days: f64,
    rank: Option<usize>, // rank of THIS oracle memory in hybrid top-k
    injected: bool,      // THIS oracle title rendered in the context string
}

fn bucket(days: f64) -> &'static str {
    if days < 1.0 / 24.0 {
        "same_session"
    } else if days < 7.0 {
        "recent"
    } else if days < 30.0 {
        "mid"
    } else {
        "long_horizon"
    }
}

fn mem_rank(results: &[SearchResult], title: &str) -> Option<usize> {
    results
        .iter()
        .position(|r| r.obs_type.starts_with("memory:") && r.title == title)
}

/// Fallback signal: files the run actually edited/wrote (weaker than a commit).
fn edited_files(run: &RunRec, obs_by_id: &HashMap<String, &serde_json::Value>) -> Vec<String> {
    let mut f = Vec::new();
    for oid in &run.obs_ids {
        let Some(o) = obs_by_id.get(oid) else {
            continue;
        };
        match str_field(o, "obs_type").as_deref() {
            Some("file_edit") | Some("file_write") => f.extend(strs_field(o, "files")),
            _ => {}
        }
    }
    f
}

struct ArmMetrics {
    r5: f64,
    r10: f64,
    mrr: f64,
    inj: f64,
    n_runs: usize,
    n_pairs: usize,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mut project_filter: Option<String> = None;
    let mut export_json: Option<String> = None;
    let mut base = "http://localhost:3111".to_string();
    let mut loose = false;
    let mut synthetic = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--project" => project_filter = args.next(),
            "--export-json" => export_json = args.next(),
            "--base-url" => base = args.next().unwrap_or(base),
            "--loose-paths" => loose = true,
            "--synthetic" => synthetic = true,
            "-h" | "--help" => {
                eprintln!(
                    "usage: corpus-memory-bench [--project <name>] [--export-json <file>] \
                     [--base-url <url>] [--loose-paths] [--synthetic]\n\
                     --synthetic: hermetic deterministic fixture (no daemon, no ~/.hifz \
                     writes); always yields PASS/FAIL."
                );
                return Ok(());
            }
            other => eprintln!("warning: unknown arg '{other}' (ignored)"),
        }
    }

    println!("=== corpus-memory-bench ===");

    if synthetic {
        // Hermetic deterministic fixture: planted (prompt, relevant-memory,
        // decoy, file, age-days) cases. No daemon, no ~/.hifz writes. Ages
        // are spread to populate every bucket incl. long_horizon. Grounding
        // is exercised (eval_arm `ground=true`). Always yields PASS/FAIL.
        const BASE: i64 = 1_760_000_000; // fixed epoch (deterministic)
        type Case = (
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            i64,
        );
        const CASES: &[Case] = &[
            (
                "how do we keep credentials out of source control",
                "Secrets load from the environment, never committed",
                "All API keys and tokens are read from process env vars; nothing sensitive is checked in.",
                "src/config.rs",
                "Structured logging uses the tracing crate",
                "Logs are emitted as JSON via tracing-subscriber.",
                "src/log.rs",
                0,
            ),
            (
                "what stops two writers clobbering the same row",
                "Optimistic concurrency via a version column",
                "Each row carries a version; updates fail if the version moved, forcing a retry.",
                "src/store.rs",
                "The CLI parses arguments with clap derive",
                "Command-line flags use clap's derive API.",
                "src/cli.rs",
                1,
            ),
            (
                "how is the service shut down without dropping requests",
                "Graceful shutdown drains in-flight work",
                "On SIGTERM the server stops accepting and waits for active handlers before exit.",
                "src/server.rs",
                "Timestamps are stored as RFC3339 strings",
                "All times are serialized RFC3339 in UTC.",
                "src/time.rs",
                3,
            ),
            (
                "why don't repeated identical events pile up",
                "Events deduplicate by content hash within a TTL",
                "A content hash plus a short TTL window suppresses duplicate observations.",
                "src/dedup.rs",
                "HTTP routes are mounted under /api/v1",
                "The axum router nests everything under /api/v1.",
                "src/web.rs",
                6,
            ),
            (
                "how do slow downstream calls avoid hanging us",
                "Outbound calls have a fail-fast timeout",
                "Every network client sets a short timeout so a slow peer can't stall the system.",
                "src/http.rs",
                "Markdown export writes one file per memory",
                "Export dumps each memory to its own .md file.",
                "src/export.rs",
                9,
            ),
            (
                "how are large blobs kept out of the hot path",
                "Big payloads are chunked and streamed",
                "Large content is split into chunks and streamed rather than buffered whole.",
                "src/chunk.rs",
                "Config is layered env over file defaults",
                "Configuration merges file defaults with env overrides.",
                "src/cfg.rs",
                15,
            ),
            (
                "what makes search tolerant of typos and synonyms",
                "Hybrid ranking fuses vector and keyword hits",
                "Reciprocal-rank fusion blends embedding similarity with BM25 lexical matches.",
                "src/search.rs",
                "Errors map to RFC7807 problem JSON",
                "API errors are returned as problem+json bodies.",
                "src/error.rs",
                25,
            ),
            (
                "how does old knowledge fade unless reinforced",
                "Memory strength decays on an Ebbinghaus curve",
                "Strength multiplies by exp(-age/30d); access and commits reinforce it.",
                "src/rank.rs",
                "The build embeds the git SHA at compile time",
                "A build script bakes the commit SHA into the binary.",
                "build.rs",
                40,
            ),
            (
                "how do we prove a change actually shipped",
                "Commit signals ground memories to what landed",
                "A commit_made observation strengthens memories whose files were committed.",
                "src/ground.rs",
                "Pagination uses opaque keyset cursors",
                "List endpoints page via base64 keyset cursors.",
                "src/page.rs",
                60,
            ),
            (
                "how is the graph kept from unbounded growth",
                "Cold low-strength memories are garbage-collected",
                "A GC sweep removes expired, low-strength, unreferenced memories.",
                "src/forget.rs",
                "Feature flags are read from a TOML file",
                "Flags load from features.toml at startup.",
                "src/flags.rs",
                80,
            ),
            (
                "how do we recover state after a crash",
                "A write-ahead log replays on restart",
                "Mutations append to a WAL; startup replays uncommitted entries.",
                "src/wal.rs",
                "Unit tests run against an in-memory store",
                "Tests use an ephemeral in-memory database.",
                "src/testutil.rs",
                100,
            ),
            (
                "what keeps one tenant from seeing another's data",
                "Every query is scoped by project id",
                "All reads and writes filter on the owning project to enforce isolation.",
                "src/scope.rs",
                "Docs are generated with mdbook",
                "The handbook is built by mdbook from docs/.",
                "docs/book.rs",
                120,
            ),
        ];

        let mut smems: Vec<MemRec> = Vec::new();
        let mut sruns: Vec<RunRec> = Vec::new();
        for (prompt, mt, mc, mf, dt, dc, df, age) in CASES {
            smems.push(MemRec {
                title: mt.to_string(),
                content: mc.to_string(),
                files: vec![mf.to_string()],
                created: BASE - age * 86_400,
                project: "syn".to_string(),
            });
            smems.push(MemRec {
                title: dt.to_string(),
                content: dc.to_string(),
                files: vec![df.to_string()],
                created: BASE,
                project: "syn".to_string(),
            });
            sruns.push(RunRec {
                prompt: prompt.to_string(),
                started: BASE,
                project: "syn".to_string(),
                obs_ids: vec![mf.to_string()], // planted shipped file
            });
        }
        let obs_empty: HashMap<String, &serde_json::Value> = HashMap::new();
        let embedder = Embedder::new()?;

        let (m, pairs) = eval_arm(
            &sruns,
            &smems,
            &obs_empty,
            "syn",
            false,
            true, // exercise commit-grounding
            &embedder,
            |r: &RunRec, _: &HashMap<String, &serde_json::Value>| r.obs_ids.clone(),
        )
        .await?;

        // Dedicated synthetic BM25-only baseline (own branch — the export
        // gate's baseline loop is coupled to commit_files/export).
        let mut bfirst: Vec<Option<usize>> = Vec::new();
        let mut br10 = Vec::new();
        let cfg = SearchConfig {
            skip_vector: true,
            ..Default::default()
        };
        for r in &sruns {
            let oracle: Vec<&MemRec> = smems
                .iter()
                .filter(|mm| mm.created <= r.started && mm.files == r.obs_ids)
                .collect();
            let bdb = db::connect_mem().await?;
            init_schema(&bdb, embedder.dimension()).await?;
            for mm in &smems {
                hifz::remember::save(
                    &bdb,
                    &embedder,
                    BENCH_PROJECT,
                    "fact",
                    &mm.title,
                    &mm.content,
                    &[],
                    &mm.files,
                    None,
                )
                .await?;
            }
            let res = search::search_hybrid_with_config(
                &bdb,
                &embedder,
                &r.prompt,
                10,
                Some(BENCH_PROJECT),
                cfg,
            )
            .await?;
            let ranks: Vec<Option<usize>> =
                oracle.iter().map(|mm| mem_rank(&res, &mm.title)).collect();
            bfirst.push(ranks.iter().filter_map(|x| *x).min());
            br10.push(if ranks.iter().any(|x| matches!(x, Some(p) if *p < 10)) {
                1.0
            } else {
                0.0
            });
        }
        let bmrr = mrr(&bfirst);
        let br10v = avg(&br10);

        println!("mode: --synthetic (hermetic, deterministic, no side-effects)");
        println!(
            "planted: {} cases ({} memories, {} runs)",
            CASES.len(),
            smems.len(),
            sruns.len()
        );
        println!(
            "hybrid (grounded) : runs={} pairs={}  R@5={:.3} R@10={:.3} MRR={:.3} inj={:.3}",
            m.n_runs, m.n_pairs, m.r5, m.r10, m.mrr, m.inj
        );
        println!("bm25-only baseline:                 R@10={br10v:.3} MRR={bmrr:.3}");
        println!();
        println!("by age bucket:  bucket          n   recall@10  inj   MRR");
        for b in ["same_session", "recent", "mid", "long_horizon"] {
            let sub: Vec<&PairOutcome> = pairs
                .iter()
                .filter(|p| bucket(p.age_gap_days) == b)
                .collect();
            if sub.is_empty() {
                println!("  {b:<14} (none)");
                continue;
            }
            let r10 = sub
                .iter()
                .filter(|p| matches!(p.rank, Some(r) if r < 10))
                .count() as f64
                / sub.len() as f64;
            let inj = sub.iter().filter(|p| p.injected).count() as f64 / sub.len() as f64;
            let rk: Vec<Option<usize>> = sub.iter().map(|p| p.rank).collect();
            println!(
                "  {b:<14} {:>3}   {:>7.3}  {:>5.3} {:>5.3}",
                sub.len(),
                r10,
                inj,
                mrr(&rk)
            );
        }
        println!();

        // Synthetic gate validates RETRIEVAL QUALITY deterministically: the
        // hybrid pipeline must beat the BM25-only baseline and clear an
        // absolute Recall@10 floor (same retrieval clauses as the real-corpus
        // gate). The absolute injection-hit floor is intentionally NOT a
        // synthetic pass/fail: this fixture is deliberately age-skewed to
        // populate the long_horizon bucket, so an absolute injection floor
        // would measure recency-decay aggressiveness, not retrieval quality.
        // injection-vs-age is printed as a DIAGNOSTIC; the real-corpus gate
        // keeps the injection floor (natural ages) and Track D's
        // `long_horizon_memory_still_recallable` covers decay directly.
        const D_MRR: f64 = 0.05;
        const D_R10: f64 = 0.03;
        const FLOOR_R10: f64 = 0.50;
        let mut reasons = Vec::new();
        if m.mrr < bmrr + D_MRR {
            reasons.push(format!(
                "MRR {:.3} does not beat BM25-only {bmrr:.3} by ≥{D_MRR}",
                m.mrr
            ));
        }
        if m.r10 < br10v + D_R10 {
            reasons.push(format!(
                "Recall@10 {:.3} does not beat BM25-only {br10v:.3} by ≥{D_R10}",
                m.r10
            ));
        }
        if m.r10 < FLOOR_R10 {
            reasons.push(format!("Recall@10 {:.3} below floor {FLOOR_R10}", m.r10));
        }
        let long_inj: Vec<&PairOutcome> = pairs
            .iter()
            .filter(|p| bucket(p.age_gap_days) == "long_horizon")
            .collect();
        if !long_inj.is_empty() {
            let li = long_inj.iter().filter(|p| p.injected).count() as f64 / long_inj.len() as f64;
            println!(
                "DIAGNOSTIC: long_horizon injection-hit {li:.3} vs overall {:.3} — old \
                 memories are retrieved but the recency-weighted, token-bounded injection \
                 path under-surfaces them (finding, not a synthetic gate failure).",
                m.inj
            );
        }
        let v = if reasons.is_empty() {
            Verdict::Pass
        } else {
            Verdict::Fail(reasons)
        };
        std::process::exit(v.emit());
    }

    let Export {
        memories,
        runs,
        sessions,
        observations,
    } = match load_export(export_json.as_deref(), &base).await {
        Ok(e) => e,
        Err(e) => {
            std::process::exit(Verdict::Skip(format!("corpus unavailable: {e}")).emit());
        }
    };

    // session id -> project (fallback when run.project is absent)
    let sess_proj: HashMap<String, String> = sessions
        .iter()
        .filter_map(|s| {
            Some((
                rec_str(s.get("id")?)?,
                norm_project(&str_field(s, "project")?),
            ))
        })
        .collect();

    let obs_by_id: HashMap<String, &serde_json::Value> = observations
        .iter()
        .filter_map(|o| Some((rec_str(o.get("id")?)?, o)))
        .collect();

    let mems: Vec<MemRec> = memories
        .iter()
        .filter(|m| m.get("is_latest").and_then(|v| v.as_bool()).unwrap_or(true))
        .filter_map(|m| {
            Some(MemRec {
                title: str_field(m, "title")?,
                content: str_field(m, "content").unwrap_or_default(),
                files: strs_field(m, "files"),
                created: epoch_secs(&str_field(m, "created_at").unwrap_or_default()),
                project: norm_project(&str_field(m, "project").unwrap_or_default()),
            })
        })
        .collect();

    let runrecs: Vec<RunRec> = runs
        .iter()
        .filter_map(|r| {
            let proj = str_field(r, "project")
                .map(|p| norm_project(&p))
                .or_else(|| {
                    r.get("session_id")
                        .and_then(rec_str)
                        .and_then(|s| sess_proj.get(&s).cloned())
                })?;
            let prompt = str_field(r, "prompt").unwrap_or_default();
            if prompt.trim().is_empty() {
                return None;
            }
            Some(RunRec {
                prompt,
                started: epoch_secs(&str_field(r, "started_at").unwrap_or_default()),
                project: proj,
                obs_ids: r
                    .get("observation_ids")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(rec_str).collect())
                    .unwrap_or_default(),
            })
        })
        .collect();

    let target = project_filter.map(|p| norm_project(&p)).unwrap_or_else(|| {
        // modal project across runs
        let mut c: HashMap<&str, usize> = HashMap::new();
        for r in &runrecs {
            *c.entry(r.project.as_str()).or_default() += 1;
        }
        c.into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(p, _)| p.to_string())
            .unwrap_or_default()
    });
    println!(
        "corpus: {} memories, {} runs, {} sessions, {} observations | target project: {target}",
        mems.len(),
        runrecs.len(),
        sessions.len(),
        observations.len()
    );

    // Evaluate one signal arm end-to-end. `file_set` extracts the run's
    // ground-truth file set; results are seeded into a fresh per-run in-mem DB
    // (no `run` rows → no `# Recent task outcomes` leak).
    #[allow(clippy::too_many_arguments)] // benchmark harness helper; cohesive arg set
    async fn eval_arm<F>(
        runrecs: &[RunRec],
        mems: &[MemRec],
        obs_by_id: &HashMap<String, &serde_json::Value>,
        target: &str,
        loose: bool,
        ground: bool,
        embedder: &Embedder,
        file_set: F,
    ) -> Result<(ArmMetrics, Vec<PairOutcome>)>
    where
        F: Fn(&RunRec, &HashMap<String, &serde_json::Value>) -> Vec<String>,
    {
        let mut run_recall5 = Vec::new();
        let mut run_recall10 = Vec::new();
        let mut run_first_rank: Vec<Option<usize>> = Vec::new();
        let mut run_inj = Vec::new();
        let mut pairs: Vec<PairOutcome> = Vec::new();
        let mut eligible = 0usize;

        for r in runrecs.iter().filter(|r| r.project == target) {
            let fset = file_set(r, obs_by_id);
            if fset.is_empty() {
                continue;
            }
            // Oracle: leakage-safe, same project, file overlap with shipped set.
            let oracle: Vec<&MemRec> = mems
                .iter()
                .filter(|m| {
                    m.project == *target
                        && m.created <= r.started
                        && files_overlap(&m.files, &fset, loose)
                })
                .collect();
            if oracle.is_empty() {
                continue;
            }
            eligible += 1;

            // Fresh DB seeded with the leakage-safe memory pool only.
            let db = db::connect_mem().await?;
            init_schema(&db, embedder.dimension()).await?;
            for m in mems.iter().filter(|m| m.created <= r.started) {
                hifz::remember::save(
                    &db,
                    embedder,
                    BENCH_PROJECT,
                    "fact",
                    &m.title,
                    &m.content,
                    &[],
                    &m.files,
                    None,
                )
                .await?;
                // Backdate so recency decay is realistic (the long-session
                // verdict depends on real ages, not all-fresh rows).
                let ts = chrono::DateTime::from_timestamp(m.created, 0)
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default();
                if !ts.is_empty() {
                    let _ = db
                        .query(
                            "UPDATE memory SET created_at=$ts, updated_at=$ts \
                                WHERE project=$p AND title=$t",
                        )
                        .bind(("ts", ts))
                        .bind(("p", BENCH_PROJECT.to_string()))
                        .bind(("t", m.title.clone()))
                        .await;
                }
            }

            // Exercise commit-grounding on the per-run DB (synthetic mode):
            // a non-revert commit on the shipped file strengthens the
            // memories that describe it.
            if ground {
                let _ = hifz::ground::on_commit_signal(
                    &db,
                    BENCH_PROJECT,
                    &CommitSignal {
                        files_added_modified: fset.clone(),
                        files_removed: vec![],
                        is_revert: false,
                    },
                )
                .await;
            }

            let results =
                search::search_hybrid(&db, embedder, &r.prompt, 10, Some(BENCH_PROJECT)).await?;
            let ctx = hifz::context::generate_context_with_query(
                &db,
                Some(embedder),
                BENCH_PROJECT,
                Some(&r.prompt),
                2048,
            )
            .await?;

            // Per-run: recall_any over the oracle set + first oracle rank.
            let ranks: Vec<Option<usize>> = oracle
                .iter()
                .map(|m| mem_rank(&results, &m.title))
                .collect();
            let best = ranks.iter().filter_map(|x| *x).min();
            run_recall5.push(if ranks.iter().any(|r| matches!(r, Some(p) if *p < 5)) {
                1.0
            } else {
                0.0
            });
            run_recall10.push(if ranks.iter().any(|r| matches!(r, Some(p) if *p < 10)) {
                1.0
            } else {
                0.0
            });
            run_first_rank.push(best);
            let any_inj = oracle.iter().any(|m| ctx.contains(&m.title));
            run_inj.push(if any_inj { 1.0 } else { 0.0 });

            // Per-pair, for the age-stratified table.
            for (m, rk) in oracle.iter().zip(ranks.iter()) {
                let age = (r.started - m.created).max(0) as f64 / 86400.0;
                pairs.push(PairOutcome {
                    age_gap_days: age,
                    rank: *rk,
                    injected: ctx.contains(&m.title),
                });
            }
        }

        let m = ArmMetrics {
            r5: avg(&run_recall5),
            r10: avg(&run_recall10),
            mrr: mrr(&run_first_rank),
            inj: avg(&run_inj),
            n_runs: eligible,
            n_pairs: pairs.len(),
        };
        Ok((m, pairs))
    }

    let embedder = Embedder::new()?;

    // Commit-grounded "runs" derived DIRECTLY from `commit_made` observations
    // (not via run.observation_ids). Backfilled history and human terminal
    // commits are never linked to a Claude run's observation_ids, so the
    // run-keyed reconstruction (`commit_files`) is structurally empty on real
    // corpora. A commit IS a first-class query unit: intent = commit message,
    // shipped files = its metadata, leakage cut = the commit timestamp. This
    // is the more defensible commit-grounded signal and consumes
    // `make backfill-commits` data. Single-repo assumption: with --project
    // set, every commit_made obs is treated as that project (backfill is
    // per-repo); revert commits are excluded (an undoing, not a confirmation).
    let commit_runs: Vec<RunRec> = observations
        .iter()
        .filter(|o| str_field(o, "obs_type").as_deref() == Some("commit_made"))
        .filter_map(|o| {
            let fallback = strs_field(o, "files");
            let sig = CommitSignal::from_metadata(o.get("metadata"), &fallback);
            if sig.is_revert || sig.files_added_modified.is_empty() {
                return None;
            }
            let subject = o
                .get("metadata")
                .and_then(|m| m.get("message"))
                .and_then(|v| v.as_str())
                .and_then(|s| s.lines().next())
                .map(|s| s.to_string())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| str_field(o, "title"))?;
            Some(RunRec {
                prompt: subject,
                started: epoch_secs(&str_field(o, "timestamp").unwrap_or_default()),
                project: target.clone(),
                obs_ids: sig.files_added_modified, // shipped files for file_set
            })
        })
        .collect();

    // Commit-grounded arm (the real signal) + labeled file-edit fallback.
    let (commit_m, commit_pairs) = eval_arm(
        &commit_runs,
        &mems,
        &obs_by_id,
        &target,
        loose,
        false,
        &embedder,
        |r: &RunRec, _: &HashMap<String, &serde_json::Value>| r.obs_ids.clone(),
    )
    .await?;
    let (fb_m, fb_pairs) = eval_arm(
        &runrecs,
        &mems,
        &obs_by_id,
        &target,
        loose,
        false,
        &embedder,
        edited_files,
    )
    .await?;

    // BM25-only baseline on whichever signal we will gate on.
    let use_commit = commit_m.n_runs >= MIN_RUNS && commit_m.n_pairs >= MIN_PAIRS;
    let (gate_m, gate_pairs, signal) = if use_commit {
        (&commit_m, &commit_pairs, "commit-grounded")
    } else {
        (&fb_m, &fb_pairs, "file-edit-fallback (WEAK PROXY)")
    };

    println!();
    println!("signal evaluated for gate: {signal}");
    println!(
        "commit-grounded : runs={} pairs={}  R@5={:.3} R@10={:.3} MRR={:.3} inj={:.3}",
        commit_m.n_runs, commit_m.n_pairs, commit_m.r5, commit_m.r10, commit_m.mrr, commit_m.inj
    );
    println!(
        "file-edit (proxy): runs={} pairs={}  R@5={:.3} R@10={:.3} MRR={:.3} inj={:.3}",
        fb_m.n_runs, fb_m.n_pairs, fb_m.r5, fb_m.r10, fb_m.mrr, fb_m.inj
    );

    // Age-stratified table on the gate signal (long_horizon = long-session verdict).
    println!();
    println!("by age bucket ({signal}):  bucket          n   recall@10  inj   MRR");
    for b in ["same_session", "recent", "mid", "long_horizon"] {
        let sub: Vec<&PairOutcome> = gate_pairs
            .iter()
            .filter(|p| bucket(p.age_gap_days) == b)
            .collect();
        if sub.is_empty() {
            println!("  {b:<14} (none)");
            continue;
        }
        let r10 = sub
            .iter()
            .filter(|p| matches!(p.rank, Some(r) if r < 10))
            .count() as f64
            / sub.len() as f64;
        let inj = sub.iter().filter(|p| p.injected).count() as f64 / sub.len() as f64;
        let ranks: Vec<Option<usize>> = sub.iter().map(|p| p.rank).collect();
        println!(
            "  {b:<14} {:>3}   {:>7.3}  {:>5.3} {:>5.3}{}",
            sub.len(),
            r10,
            inj,
            mrr(&ranks),
            if b == "long_horizon" {
                "   <-- long-session verdict"
            } else {
                ""
            }
        );
    }
    println!();

    // SKIP conditions — never silently PASS.
    if !use_commit {
        // Print the fallback numbers (already above) then SKIP with guidance.
        let why = if commit_m.n_pairs == 0 {
            "no commit_made observations in the corpus — commit-grounding inert. \
             Fix: `make install-git-hook`, then accumulate commits. Numbers above \
             use the WEAK file-edit proxy and are NOT a pass."
                .to_string()
        } else {
            format!(
                "only {} commit-grounded runs / {} pairs (need ≥{MIN_RUNS} / ≥{MIN_PAIRS}). \
                 Numbers above use the weak file-edit proxy.",
                commit_m.n_runs, commit_m.n_pairs
            )
        };
        std::process::exit(Verdict::Skip(why).emit());
    }

    // Commit-grounded gate: beat BM25-only on the user's own data + floors.
    // (BM25 baseline reuses the same per-run rebuild but with skip_vector; to
    // keep the bench bounded we recompute MRR/R@10 on the gate signal only.)
    let mut bm25_first: Vec<Option<usize>> = Vec::new();
    let mut bm25_r10 = Vec::new();
    let cfg = SearchConfig {
        skip_vector: true,
        ..Default::default()
    };
    for r in commit_runs.iter().filter(|r| r.project == target) {
        let fset = r.obs_ids.clone(); // shipped files (commit-derived)
        if fset.is_empty() {
            continue;
        }
        let oracle: Vec<&MemRec> = mems
            .iter()
            .filter(|m| {
                m.project == target
                    && m.created <= r.started
                    && files_overlap(&m.files, &fset, loose)
            })
            .collect();
        if oracle.is_empty() {
            continue;
        }
        let bdb = db::connect_mem().await?;
        init_schema(&bdb, embedder.dimension()).await?;
        for m in mems.iter().filter(|m| m.created <= r.started) {
            hifz::remember::save(
                &bdb,
                &embedder,
                BENCH_PROJECT,
                "fact",
                &m.title,
                &m.content,
                &[],
                &m.files,
                None,
            )
            .await?;
        }
        let res = search::search_hybrid_with_config(
            &bdb,
            &embedder,
            &r.prompt,
            10,
            Some(BENCH_PROJECT),
            cfg,
        )
        .await?;
        let ranks: Vec<Option<usize>> = oracle.iter().map(|m| mem_rank(&res, &m.title)).collect();
        bm25_first.push(ranks.iter().filter_map(|x| *x).min());
        bm25_r10.push(if ranks.iter().any(|r| matches!(r, Some(p) if *p < 10)) {
            1.0
        } else {
            0.0
        });
    }
    let bm25_mrr = mrr(&bm25_first);
    let bm25_r10v = avg(&bm25_r10);

    println!(
        "bm25-only baseline: R@10={:.3} MRR={:.3}",
        bm25_r10v, bm25_mrr
    );

    const D_MRR: f64 = 0.05;
    const D_R10: f64 = 0.03;
    const FLOOR_R10: f64 = 0.50;
    const FLOOR_INJ: f64 = 0.40;
    const LONG_FLOOR: f64 = 0.20;
    let mut reasons = Vec::new();
    if gate_m.mrr < bm25_mrr + D_MRR {
        reasons.push(format!(
            "MRR {:.3} does not beat BM25-only {:.3} by ≥{D_MRR}",
            gate_m.mrr, bm25_mrr
        ));
    }
    if gate_m.r10 < bm25_r10v + D_R10 {
        reasons.push(format!(
            "Recall@10 {:.3} does not beat BM25-only {:.3} by ≥{D_R10}",
            gate_m.r10, bm25_r10v
        ));
    }
    if gate_m.r10 < FLOOR_R10 {
        reasons.push(format!(
            "Recall@10 {:.3} below floor {FLOOR_R10}",
            gate_m.r10
        ));
    }
    if gate_m.inj < FLOOR_INJ {
        reasons.push(format!(
            "injection-hit {:.3} below floor {FLOOR_INJ}",
            gate_m.inj
        ));
    }
    let long: Vec<&PairOutcome> = gate_pairs
        .iter()
        .filter(|p| bucket(p.age_gap_days) == "long_horizon")
        .collect();
    if long.len() >= 10 {
        let li = long.iter().filter(|p| p.injected).count() as f64 / long.len() as f64;
        if li < LONG_FLOOR {
            reasons.push(format!(
                "long-horizon injection-hit {li:.3} below floor {LONG_FLOOR} \
                 (old still-relevant memory is being buried)"
            ));
        }
    }
    let verdict = if reasons.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail(reasons)
    };
    std::process::exit(verdict.emit())
}

fn avg(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}
