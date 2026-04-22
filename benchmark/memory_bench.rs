//! Memory-tier benchmark for hifz.
//!
//! Complements `longmemeval-bench` (which tests observation retrieval) by
//! measuring the **memory tier** (the `hifz` table) and context injection:
//! does saving insights via `hifz_save` and recalling them via `hifz_search`
//! actually surface the right row?
//!
//! Deterministic, no external dataset — fixtures are generated in-process
//! from a seed list of (topic, fact, probe) triples, each probe rephrased a
//! few ways to simulate how a user might ask about the fact later.
//!
//! Metrics:
//!   - Recall@5 / Recall@10 / MRR over `search_hybrid`
//!   - Injection hit-rate: does `generate_context_with_query(...)` render the
//!     oracle memory's title?
//!
//! Modes:
//!   - `base`    — BM25 + vector + RRF, strength-only (Phase-0 baseline)
//!   - `full`    — Phase 1 stack: project-scoped, richer-text embed,
//!                 strength·recency·access, query-aware injection
//!
//! Usage:
//!   cargo run --release --bin memory-bench -- full
//!   cargo run --release --bin memory-bench -- base

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;

use hifz::db::{self, init_schema};
use hifz::embed::Embedder;
use hifz::llm_rerank;
use hifz::ollama::OllamaClient;
use hifz::remember;
use hifz::rerank::{Reranker, RerankerChoice};
use hifz::search::{self, SearchConfig};

/// Which reranker path the bench is asked to use.
enum RerankSpec {
    None,
    /// In-process fastembed cross-encoder.
    Fastembed(RerankerChoice),
    /// Listwise LLM rerank via local Ollama, model name verbatim.
    Llm(String),
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

struct Triple {
    title: &'static str,
    content: &'static str,
    concepts: &'static [&'static str],
    files: &'static [&'static str],
    probes: &'static [&'static str],
}

/// Seed fixture: 30 diverse (title, content, probes) triples. Each probe is a
/// rephrasing a user might reasonably ask to retrieve this memory.
/// Deterministic — keep ordering stable so metrics are comparable run to run.
const FIXTURES: &[Triple] = &[
    Triple {
        title: "Never mock the database in integration tests",
        content: "We got burned last quarter when mocked tests passed but the prod migration failed. Integration tests must hit a real DB.",
        concepts: &["testing", "database", "integration"],
        files: &["tests/integration.rs"],
        probes: &[
            "should integration tests mock the database?",
            "policy on mocking DB in tests",
            "lesson from the failed migration about mocks",
        ],
    },
    Triple {
        title: "Use sqlx for Postgres, not diesel",
        content: "Decided in Q2 — sqlx's compile-time query checks matched our schema-first workflow better than diesel's DSL.",
        concepts: &["postgres", "sqlx", "orm"],
        files: &["Cargo.toml", "src/db.rs"],
        probes: &[
            "which postgres library do we use?",
            "why we picked sqlx over diesel",
            "ORM choice for this project",
        ],
    },
    Triple {
        title: "auth middleware rewrite driven by compliance",
        content: "Legal flagged session token storage as non-compliant with the new standard; rewrite must meet legal requirements, not just tidy code.",
        concepts: &["auth", "compliance", "tokens"],
        files: &["src/middleware/auth.rs"],
        probes: &[
            "why are we rewriting the auth middleware?",
            "compliance reason behind the auth changes",
            "session token storage issue",
        ],
    },
    Triple {
        title: "Merge freeze before mobile release",
        content: "All non-critical merges are frozen after Thursday 2026-03-05 because the mobile team is cutting a release branch.",
        concepts: &["release", "freeze", "mobile"],
        files: &[],
        probes: &[
            "when does the merge freeze start?",
            "why can't I merge next week?",
            "mobile release branch timing",
        ],
    },
    Triple {
        title: "HNSW index uses cosine distance",
        content: "All vector indexes in hifz use DIST COSINE with 384-dim fastembed vectors; swapping to euclidean requires re-indexing.",
        concepts: &["hnsw", "vector", "cosine"],
        files: &["src/db.rs"],
        probes: &[
            "what distance does the vector index use?",
            "cosine or euclidean in hifz?",
            "how big are the embeddings?",
        ],
    },
    Triple {
        title: "SurrealDB lacks math::exp and time::diff",
        content: "Recency decay math must run Rust-side; SurrealQL has neither math::exp nor time::diff. Verified in surrealdb/core/src/fnc.",
        concepts: &["surrealdb", "scoring", "limitations"],
        files: &["src/rank.rs"],
        probes: &[
            "can I compute exp in surrealql?",
            "why is scoring done in rust?",
            "surrealdb missing math functions",
        ],
    },
    Triple {
        title: "RELATE UNIQUE enforces only (in, out)",
        content: "Per-via uniqueness on mem_link has to be done Rust-side — RELATE UNIQUE only dedupes on the (in, out) pair.",
        concepts: &["surrealdb", "graph", "relate"],
        files: &["src/link.rs"],
        probes: &[
            "does RELATE UNIQUE cover edge fields?",
            "how to dedupe graph edges per via",
            "surrealdb unique relate limitation",
        ],
    },
    Triple {
        title: "Pre-commit hooks run cargo fmt and clippy",
        content: "Never bypass hooks with --no-verify; if a hook fails, fix the issue and create a new commit rather than amending.",
        concepts: &["git", "hooks", "workflow"],
        files: &[".git/hooks/pre-commit"],
        probes: &[
            "what do our pre-commit hooks do?",
            "can I use --no-verify?",
            "how to handle a failing pre-commit",
        ],
    },
    Triple {
        title: "Embedder is a 384-dim AllMiniLML6V2",
        content: "Local fastembed model; no prefix needed for MiniLM. Batch size 64 in embed_batch.",
        concepts: &["embeddings", "fastembed", "model"],
        files: &["src/embed.rs"],
        probes: &[
            "which embedding model does hifz use?",
            "fastembed model dimensions",
            "do we need a prompt prefix for embeddings?",
        ],
    },
    Triple {
        title: "Memories are project-scoped in Phase 1a",
        content: "hifz rows carry a project column; search filters by project OR project='global'. Backfill via `hifz reindex` derives project from session_ids[0].",
        concepts: &["memories", "scoping", "project"],
        files: &["src/db.rs", "src/reindex.rs"],
        probes: &[
            "are memories cross-project?",
            "how does project scoping work for hifz",
            "what does hifz reindex do?",
        ],
    },
    Triple {
        title: "Recency decay half-life is 30 days",
        content: "Mem0 formula: score = strength * exp(-age_days / 30) * (1 + 0.1 * min(access, 20)).",
        concepts: &["scoring", "recency", "mem0"],
        files: &["src/rank.rs"],
        probes: &[
            "what is the recency half-life?",
            "mem0 scoring formula",
            "how is access_count used in ranking?",
        ],
    },
    Triple {
        title: "Core memory is a per-project singleton",
        content: "hifz_core stores identity, goals, invariants, watchlist — always prepended above # Saved memories in injected context.",
        concepts: &["core", "memgpt", "context"],
        files: &["src/core_mem.rs"],
        probes: &[
            "where do I store invariants?",
            "core memory structure",
            "memgpt-style always-on block",
        ],
    },
    Triple {
        title: "Ollama is optional, off by default",
        content: "hifz runs zero-LLM by default; Ollama is opt-in for consolidation and evolution. Deterministic paths always work without it.",
        concepts: &["ollama", "llm", "configuration"],
        files: &["src/main.rs", "src/ollama.rs"],
        probes: &[
            "do I need ollama to run hifz?",
            "when does hifz call an llm?",
            "can hifz run fully local?",
        ],
    },
    Triple {
        title: "Hooks capture 12 lifecycle events",
        content: "SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PreCompact, etc. Payloads POST to /hifz/observe.",
        concepts: &["hooks", "capture", "events"],
        files: &["plugin/hooks/hooks.json"],
        probes: &[
            "what events does hifz hook?",
            "where do hook payloads go?",
            "list of claude code hooks hifz uses",
        ],
    },
    Triple {
        title: "Consolidation has four tiers",
        content: "Tier 1 semantic (LLM), Tier 2 reflect (no LLM, concept clustering), Tier 3 procedural (LLM), Tier 4 decay.",
        concepts: &["consolidation", "tiers"],
        files: &["src/consolidate.rs"],
        probes: &[
            "how does hifz consolidate memories?",
            "what are the consolidation tiers?",
            "does consolidation require an llm?",
        ],
    },
    Triple {
        title: "Dedup uses a 5-minute SHA window",
        content: "observe.rs skips duplicates keyed by SHA-256(session_id:tool_name:input[:500]) within the last 300s.",
        concepts: &["dedup", "observe"],
        files: &["src/dedup.rs", "src/observe.rs"],
        probes: &[
            "how long is the dedup window?",
            "dedup key in hifz observe",
            "why does hifz drop duplicate observations?",
        ],
    },
    Triple {
        title: "Evolution is gated by HIFZ_LLM_EVOLVE",
        content: "A-MEM style memory evolution — when on, the LLM mutates neighbour tags/context/links on new memory write; off by default.",
        concepts: &["evolution", "a-mem", "llm"],
        files: &["src/evolve.rs"],
        probes: &[
            "how do I turn on memory evolution?",
            "what is a-mem style evolution?",
            "hifz llm flag for evolution",
        ],
    },
    Triple {
        title: "MMR-lite diversifies on (mem_type, first concept)",
        content: "Phase 1c dedup drops redundant results sharing both the same memory type and the same leading concept. Cosine-based MMR is a later upgrade.",
        concepts: &["mmr", "ranking", "diversification"],
        files: &["src/context.rs"],
        probes: &[
            "how does hifz diversify search results?",
            "mmr-lite implementation details",
            "deduplication rule for context injection",
        ],
    },
    Triple {
        title: "Session table backs project attribution",
        content: "session.project is the authoritative source of truth; other tables backfill project from their originating session.",
        concepts: &["session", "project", "attribution"],
        files: &["src/db.rs", "src/reindex.rs"],
        probes: &[
            "which table owns the project field?",
            "how is project propagated to memories?",
            "session.project source of truth",
        ],
    },
    Triple {
        title: "Forget GC runs TTL + contradiction + low-value sweeps",
        content: "forget.rs deletes expired rows (forget_after < now), flags contradicted entries via Jaccard on concepts, and drops observations older than 180d with importance <= 2.",
        concepts: &["forget", "gc", "cleanup"],
        files: &["src/forget.rs"],
        probes: &[
            "what does hifz forget do?",
            "how are low-value memories pruned?",
            "contradiction detection mechanism",
        ],
    },
    Triple {
        title: "Fastembed requires 64MB thread stack",
        content: "main.rs sets thread_stack_size(64MB) because SurrealDB's HNSW call stack overflows under opt-level 0 otherwise.",
        concepts: &["runtime", "fastembed", "threading"],
        files: &["src/main.rs"],
        probes: &[
            "why the large thread stack size?",
            "tokio runtime config in hifz",
            "hnsw stack overflow workaround",
        ],
    },
    Triple {
        title: "Embeddings live on observations AND now memories",
        content: "Phase 1a added HNSW on hifz.embedding. Previously only observations had embeddings, so semantic recall of saved insights didn't work.",
        concepts: &["embeddings", "schema", "phase1"],
        files: &["src/db.rs"],
        probes: &[
            "are saved memories embedded?",
            "when did memory embeddings arrive?",
            "hnsw index on hifz table",
        ],
    },
    Triple {
        title: "SurrealDB uses @N@ for BM25 search, not @N,OR@",
        content: "Verified in language-tests: the canonical form is `col @1@ 'term'`. hifz historically uses `@1,OR@` which works but is non-canonical.",
        concepts: &["surrealdb", "bm25", "syntax"],
        files: &["src/search.rs"],
        probes: &[
            "surrealdb bm25 query syntax",
            "what does @1@ mean in surrealql?",
            "is @1,OR@ valid surrealdb?",
        ],
    },
    Triple {
        title: "Token budget default is 1500 for pre-compact, 2048 for session-start",
        content: "context.rs honours a configurable token_budget. Defaults keep injection compact enough to survive compaction.",
        concepts: &["context", "tokens", "budget"],
        files: &["src/context.rs", "src/config.rs"],
        probes: &[
            "default token budget for context",
            "hifz injection size limits",
            "configuration for context token budget",
        ],
    },
    Triple {
        title: "Synthetic compression runs by default at 50% confidence",
        content: "compress.rs's deterministic path labels its output 0.5 confidence. LLM compression (Ollama) can raise this but is off by default.",
        concepts: &["compression", "confidence"],
        files: &["src/compress.rs"],
        probes: &[
            "what's the default compression confidence?",
            "synthetic vs llm compression in hifz",
            "compress.rs confidence value",
        ],
    },
    Triple {
        title: "Plugin ships with hooks.json and recall SKILL",
        content: "plugin/hooks/hooks.json wires every Claude Code hook; plugin/skills/recall/SKILL.md documents hifz_recall usage.",
        concepts: &["plugin", "hooks", "skill"],
        files: &["plugin/hooks/hooks.json", "plugin/skills/recall/SKILL.md"],
        probes: &[
            "where are the claude code hooks defined?",
            "recall skill documentation",
            "hifz plugin contents",
        ],
    },
    Triple {
        title: "Search results are diversified to 3 per session",
        content: "search_hybrid caps observations from any one session to 3 in the final output, preserving diversity of context.",
        concepts: &["search", "diversity", "sessions"],
        files: &["src/search.rs"],
        probes: &[
            "max results per session in search",
            "session diversification cap",
            "why do I see only 3 from one session?",
        ],
    },
    Triple {
        title: "SurrealKV is the default storage",
        content: "Persistent mode uses SurrealKV under db_path; --memory switches to in-memory for ephemeral tests.",
        concepts: &["storage", "surrealkv"],
        files: &["src/db.rs", "src/main.rs"],
        probes: &[
            "what storage engine does hifz use?",
            "how to run hifz with ephemeral storage",
            "surrealkv vs memory mode",
        ],
    },
    Triple {
        title: "Research and architecture docs live under docs/",
        content: "docs/research/memory-architecture.md holds prior art + eval plan; docs/architecture/memory.md holds mermaid diagrams and phase status.",
        concepts: &["docs", "architecture"],
        files: &[
            "docs/research/memory-architecture.md",
            "docs/architecture/memory.md",
        ],
        probes: &[
            "where is the memory architecture documented?",
            "hifz design docs location",
            "prior art doc for hifz memory",
        ],
    },
    Triple {
        title: "Access count reinforces frequently-used memories",
        content: "On each retrieval hit, hifz increments access_count and updates last_accessed_at; rank boost caps at +2.0 at 20 accesses.",
        concepts: &["access", "ranking", "reinforcement"],
        files: &["src/search.rs", "src/rank.rs"],
        probes: &[
            "what happens when a memory is accessed?",
            "does frequent use boost a memory?",
            "access_count reinforcement cap",
        ],
    },
];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

const PROJECT: &str = "memory-bench";

struct Probe {
    text: String,
    oracle_title: String,
}

fn build_probes() -> Vec<Probe> {
    let mut probes = Vec::new();
    for f in FIXTURES {
        for p in f.probes {
            probes.push(Probe {
                text: p.to_string(),
                oracle_title: f.title.to_string(),
            });
        }
    }
    probes
}

async fn seed_memories(db: &surrealdb::Surreal<hifz::db::Db>, embedder: &Embedder) -> Result<()> {
    for f in FIXTURES {
        let concepts: Vec<String> = f.concepts.iter().map(|s| s.to_string()).collect();
        let files: Vec<String> = f.files.iter().map(|s| s.to_string()).collect();
        remember::save(
            db, embedder, PROJECT, "fact", f.title, f.content, &concepts, &files, None,
        )
        .await?;
    }
    Ok(())
}

/// Position of the oracle memory in the result pool, or `None` if absent.
fn rank_of(results: &[hifz::models::SearchResult], oracle_title: &str) -> Option<usize> {
    results
        .iter()
        .position(|r| r.obs_type.starts_with("memory:") && r.title == oracle_title)
}

/// Which memory titles (in rank order) sit *above* the oracle in the result
/// pool. Useful for diagnosing misses — if the oracle is rank 8 and the top 7
/// are all legitimate alternate answers, the bench miss was fixture-ambiguity.
/// If they're semantically unrelated, the hybrid scorer is genuinely wrong.
fn competitors_above(
    results: &[hifz::models::SearchResult],
    oracle_rank: Option<usize>,
    n: usize,
) -> Vec<(String, f64)> {
    let upper = match oracle_rank {
        Some(r) => r.min(results.len()),
        None => results.len(),
    };
    results
        .iter()
        .take(upper)
        .filter(|r| r.obs_type.starts_with("memory:"))
        .take(n)
        .map(|r| (r.title.clone(), r.score.unwrap_or(0.0)))
        .collect()
}

/// Parse a comma-separated ablation list into a SearchConfig.
/// Recognised: vector, recency, access, graph, diversify.
fn parse_ablations(raw: &str) -> SearchConfig {
    let mut cfg = SearchConfig::default();
    for part in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match part {
            "vector" => cfg.skip_vector = true,
            "recency" | "access" | "recency+access" => {
                cfg.skip_recency_access = true;
            }
            "graph" => cfg.skip_graph = true,
            "diversify" => cfg.skip_diversify = true,
            other => eprintln!("warning: unknown ablation '{other}' (ignored)"),
        }
    }
    cfg
}

fn recall_at_k(ranks: &[Option<usize>], k: usize) -> f64 {
    let hits = ranks
        .iter()
        .filter(|r| matches!(r, Some(pos) if *pos < k))
        .count();
    hits as f64 / ranks.len() as f64
}

fn mrr(ranks: &[Option<usize>]) -> f64 {
    let sum: f64 = ranks
        .iter()
        .map(|r| match r {
            Some(pos) => 1.0 / (*pos as f64 + 1.0),
            None => 0.0,
        })
        .sum();
    sum / ranks.len() as f64
}

async fn run(mode: &str, ablations: &str, rrf_k: Option<u64>, rerank: RerankSpec) -> Result<()> {
    let start = Instant::now();
    let mut cfg = parse_ablations(ablations);
    if let Some(k) = rrf_k {
        cfg.rrf_k = k;
    }
    println!("mode: {mode}");
    if !ablations.is_empty() {
        println!("ablations: {ablations}");
    }
    if let Some(k) = rrf_k {
        println!("rrf_k: {k} (default {})", SearchConfig::default().rrf_k);
    }
    match &rerank {
        RerankSpec::None => {}
        RerankSpec::Fastembed(c) => {
            println!("rerank: fastembed/{} (cross-encoder, top-20)", c.as_str())
        }
        RerankSpec::Llm(m) => {
            let url =
                std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".into());
            println!("rerank: LLM via Ollama ({m} @ {url}, listwise top-20)")
        }
    }
    println!("fixtures: {} memories, {} probes", FIXTURES.len(), {
        let n: usize = FIXTURES.iter().map(|f| f.probes.len()).sum();
        n
    });

    let db = db::connect_mem().await?;
    let embedder = Arc::new(Embedder::new()?);
    init_schema(&db, embedder.dimension()).await?;

    // Build reranker state. Only one path is active at a time — the enum
    // guarantees the bench flag is self-consistent.
    let mut fastembed_reranker: Option<Reranker> = None;
    let mut llm_reranker: Option<OllamaClient> = None;
    match rerank {
        RerankSpec::None => {}
        RerankSpec::Fastembed(choice) => {
            let t = Instant::now();
            let rr = Reranker::new(choice)?;
            println!("reranker loaded in {:?}", t.elapsed());
            fastembed_reranker = Some(rr);
        }
        RerankSpec::Llm(model) => {
            let url = std::env::var("OLLAMA_URL").ok();
            let client = OllamaClient::new(url, Some(model));
            if !client.is_available().await {
                eprintln!(
                    "warning: Ollama not reachable at the configured URL; running without rerank"
                );
            } else {
                llm_reranker = Some(client);
            }
        }
    }

    seed_memories(&db, &embedder).await?;
    println!("seeded in {:?}", start.elapsed());

    let probes = build_probes();
    let mut ranks: Vec<Option<usize>> = Vec::with_capacity(probes.len());
    let mut result_pool_size: Vec<usize> = Vec::with_capacity(probes.len());
    let mut competitors_per_probe: Vec<Vec<(String, f64)>> = Vec::with_capacity(probes.len());
    let mut inj_hits = 0usize;
    let mut rerank_elapsed = std::time::Duration::ZERO;

    let search_start = Instant::now();
    for p in &probes {
        let project_filter = match mode {
            "full" => Some(PROJECT),
            _ => None,
        };

        let mut results =
            search::search_hybrid_with_config(&db, &embedder, &p.text, 20, project_filter, cfg)
                .await?;
        if let Some(rr) = fastembed_reranker.as_ref() {
            let rr_t = Instant::now();
            results = search::apply_rerank(rr, &p.text, results, 20);
            rerank_elapsed += rr_t.elapsed();
        } else if let Some(ollama) = llm_reranker.as_ref() {
            let rr_t = Instant::now();
            results = llm_rerank::apply_llm_rerank(ollama, &p.text, results, 20).await;
            rerank_elapsed += rr_t.elapsed();
        }
        result_pool_size.push(results.len());
        let rank = rank_of(&results, &p.oracle_title);
        ranks.push(rank);
        competitors_per_probe.push(competitors_above(&results, rank, 3));

        // Injection hit-rate: does context render the oracle title?
        let ctx = hifz::context::generate_context_with_query(
            &db,
            Some(&embedder),
            PROJECT,
            Some(&p.text),
            2048,
        )
        .await?;
        if ctx.contains(&p.oracle_title) {
            inj_hits += 1;
        }
    }
    let search_elapsed = search_start.elapsed();

    let r5 = recall_at_k(&ranks, 5);
    let r10 = recall_at_k(&ranks, 10);
    let r20 = recall_at_k(&ranks, 20);
    let mrr_v = mrr(&ranks);
    let inj = inj_hits as f64 / probes.len() as f64;
    let avg_pool = result_pool_size.iter().sum::<usize>() as f64 / probes.len() as f64;

    println!();
    println!("=== memory-bench: {mode} ===");
    println!("probes         : {}", probes.len());
    println!("Recall@5       : {:.3}", r5);
    println!("Recall@10      : {:.3}", r10);
    println!("Recall@20      : {:.3}", r20);
    println!("MRR            : {:.3}", mrr_v);
    println!("Injection@top  : {:.3}", inj);
    println!("avg pool size  : {:.1}", avg_pool);
    println!("search time    : {:?}", search_elapsed);
    if fastembed_reranker.is_some() || llm_reranker.is_some() {
        println!(
            "rerank time    : {:?}  ({:.1} ms/probe avg)",
            rerank_elapsed,
            rerank_elapsed.as_secs_f64() * 1000.0 / probes.len() as f64
        );
    }
    println!("total time     : {:?}", start.elapsed());

    // Diagnostic: per-miss report with the memories that ranked above the
    // oracle so we can distinguish fixture ambiguity from real ranking bugs.
    struct Miss {
        probe_text: String,
        oracle: String,
        rank: Option<usize>,
        pool: usize,
        competitors: Vec<(String, f64)>,
    }
    let mut misses: Vec<Miss> = ranks
        .iter()
        .zip(probes.iter())
        .zip(result_pool_size.iter())
        .zip(competitors_per_probe.iter())
        .filter_map(|(((r, p), pool), comps)| {
            let is_miss = matches!(r, Some(pos) if *pos >= 5) || r.is_none();
            if !is_miss {
                return None;
            }
            Some(Miss {
                probe_text: p.text.clone(),
                oracle: p.oracle_title.clone(),
                rank: *r,
                pool: *pool,
                competitors: comps.clone(),
            })
        })
        .collect();
    misses.sort_by_key(|m| {
        (
            m.rank.is_none(),
            m.rank.unwrap_or(usize::MAX),
            m.oracle.clone(),
        )
    });

    if !misses.is_empty() {
        println!("\nmisses (not in top-5, with diagnostic rank and competitors above):");
        for m in &misses {
            let rank_str = match m.rank {
                Some(r) => format!("rank {} of {}", r + 1, m.pool),
                None => format!("NOT IN POOL (pool={})", m.pool),
            };
            println!("  - [{rank_str}] {}", m.oracle);
            println!("      probe: \"{}\"", m.probe_text);
            if !m.competitors.is_empty() {
                println!("      competing memories above oracle:");
                for (t, s) in &m.competitors {
                    println!("        * {t}  (score={s:.4})");
                }
            }
        }
    }

    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hifz=warn,memory_bench=info".into()),
        )
        .init();

    // Usage: memory-bench [base|full] [--ablate=...] [--rrf-k=N] [--rerank=<spec>]
    //
    // `--rerank=<spec>` has three shapes:
    //   * `bge-base` | `bge-v2-m3` | `jina-v1-turbo` | `jina-v2-multilingual`
    //       → in-process fastembed cross-encoder (ONNX, CPU).
    //   * `llm:<ollama-model>`  (e.g. `llm:qwen3:8b`, `llm:qwen2.5:3b`)
    //       → listwise rerank through local Ollama. Model string after the
    //         first colon is passed verbatim as the Ollama tag.
    //         `OLLAMA_URL` env (default http://localhost:11434) picks host.
    //   * `off` | omitted → no reranker.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = "full".to_string();
    let mut ablations = String::new();
    let mut rrf_k: Option<u64> = None;
    let mut rerank = RerankSpec::None;
    for a in args {
        if let Some(rest) = a.strip_prefix("--ablate=") {
            ablations = rest.to_string();
        } else if let Some(rest) = a.strip_prefix("--rrf-k=") {
            match rest.parse::<u64>() {
                Ok(k) if k > 0 => rrf_k = Some(k),
                _ => {
                    eprintln!("warning: --rrf-k expects a positive integer, got '{rest}' (ignored)")
                }
            }
        } else if let Some(rest) = a.strip_prefix("--rerank=") {
            rerank = parse_rerank_spec(rest);
        } else if matches!(a.as_str(), "full" | "base") {
            mode = a;
        } else if a == "--help" || a == "-h" {
            eprintln!(
                "usage: memory-bench [base|full] [--ablate=vector,recency,graph,diversify] \
                 [--rrf-k=N] [--rerank=<spec>]\n\
                 \n\
                 --rerank spec:\n  \
                   bge-base | bge-v2-m3 | jina-v1-turbo | jina-v2-multilingual  \
                 (fastembed cross-encoder, in-process ONNX)\n  \
                   llm:<ollama-model>  e.g. llm:qwen3:8b  \
                 (listwise rerank via Ollama; OLLAMA_URL env picks host)\n  \
                   off (default)"
            );
            return Ok(());
        } else {
            eprintln!("warning: unknown argument '{a}' (ignored)");
        }
    }

    run(&mode, &ablations, rrf_k, rerank).await
}

fn parse_rerank_spec(raw: &str) -> RerankSpec {
    if raw.is_empty() || raw == "off" {
        return RerankSpec::None;
    }
    if let Some(model) = raw.strip_prefix("llm:") {
        if model.is_empty() {
            eprintln!("warning: --rerank=llm: requires a model name (e.g. llm:qwen3:8b); ignored");
            return RerankSpec::None;
        }
        return RerankSpec::Llm(model.to_string());
    }
    if let Some(choice) = RerankerChoice::from_str(raw) {
        return RerankSpec::Fastembed(choice);
    }
    eprintln!(
        "warning: unknown --rerank value '{raw}' (expected bge-base, bge-v2-m3, \
         jina-v1-turbo, jina-v2-multilingual, llm:<ollama-model>, or off)"
    );
    RerankSpec::None
}
