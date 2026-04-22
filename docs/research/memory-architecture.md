# hifz Memory Architecture — Research

This doc captures the *why* behind hifz's memory design: prior art, tradeoffs, and the verified SurrealDB syntax we rely on. Companion to `ARCHITECTURE.md` which describes the *how*.

## Goal

Turn hifz from log-with-search into a system whose injected context the LLM consistently reaches for. Bias: deterministic / local-first. **LLM only runs when `HIFZ_LLM_EVOLVE=true`** and only for A-MEM–style Memory Evolution (on new-memory write, select neighbours and rewrite their metadata). All retrieval, linking, and injection work without an LLM.

## A. Prior art synthesis

| System | What we borrow | What we don't |
|---|---|---|
| **A-MEM** ([arxiv:2502.12110](https://arxiv.org/pdf/2502.12110)) | Memory Evolution: on new-memory write, LLM mutates *neighbours'* tags/context/links. Zettelkasten-style graph. | Always-on LLM in the write path — we make it opt-in. |
| **Mem0** ([arxiv:2504.19413](https://arxiv.org/pdf/2504.19413)) | BM25 + vector + graph RRF; recency decay `exp(-age/30)`; access reinforcement `+0.1 per retrieval, cap +2.0`. | Their proprietary orchestrator; we use SurrealDB RRF. |
| **MemGPT** ([arxiv:2310.08560](https://arxiv.org/abs/2310.08560)) | "Core memory" tier always prepended. | OS-style paging between tiers — hifz doesn't need it given SurrealDB is local. |
| **MIRIX** ([Agent-Memory-Paper-List](https://github.com/Shichun-Liu/Agent-Memory-Paper-List)) | Typed memory modules (semantic / procedural / episodic) — hifz already has `semantic_hifz`, `procedural_hifz`; we add `run` for task-scoped trajectories (episodic layer). | Full multimodal / resource / knowledge-vault tiers. |
| **Position: Episodic Memory is Missing** ([arxiv:2502.06975](https://arxiv.org/pdf/2502.06975)) | Task-scoped **run** as the unit of replay (paper: “episode”). | Separate episodic index per agent persona. |
| **MemoryBank** | Ebbinghaus forgetting curve — informs the `exp(-age/30)` default. | |

## B. Design tradeoffs

- **Deterministic graph vs LLM-proposed links.** Base graph is KNN + set-overlap. LLM evolution layers on top as opt-in. Retrieval doesn't require creativity; it must be fast and free. The LLM earns its cost rewriting neighbour metadata, not deciding edges.
- **Per-via edge tables vs single `mem_link` with `via` field.** Single table for join simplicity; `UNIQUE` on `RELATE` only covers `(in, out)`, so per-`via` uniqueness is enforced in Rust. Four-table alternative would give native uniqueness at 4× schema cost.
- **Rust-side scoring.** Forced by the fact that SurrealDB has neither `math::exp` nor `time::diff`. Side benefit: scoring experiments don't require schema migrations.
- **Async evolution.** Write path commits deterministically, evolution runs in a background queue after the dedup window. Write latency stays local. Evolution output is JSON-capped so misbehaving prompts can't corrupt the graph.
- **`RELATE UNIQUE` limitation** (`(in, out)` only) is the biggest footgun — documented, with Rust dedup as the mitigation.
- **Threshold values:** MMR 0.85, KNN cosine-distance 0.25 (`via='embedding'`), Jaccard 0.3 (concept/file). RRF k=10 (literature default is 60; sharper curve suits personal-memory corpora). Override via `SearchConfig::rrf_k`.

## C. Verified SurrealDB syntax reference

All patterns were verified against local copies of the SurrealDB source and the `hadith` production codebase before the plan was committed.

| What | Verified syntax | Source |
|---|---|---|
| Typed relation table | `DEFINE TABLE x SCHEMAFULL TYPE RELATION IN a OUT b` | `surrealdb/language-tests/.../7133_parent_dml_subquery_prepare.surql:60-61` |
| RELATE with fields | `RELATE a->edge->b SET field=...` | `hadith/src/ingest/semantic.rs` |
| RELATE idempotent | `RELATE a->edge->b UNIQUE SET ...` — `(in, out)` only | `surrealdb/core/src/syn/parser/test/stmt.rs:121` |
| Forward traversal | `SELECT ->edge->node.{fields} AS x FROM $id` | `hadith/src/web/handlers.rs:156-160` |
| Reverse traversal | `SELECT <-edge<-node.*` | `hadith/src/tools.rs` |
| Edge metadata (separate query) | `SELECT in, out, <fields> FROM edge_table WHERE in IN $ids` | `hadith/src/analysis/isnad_graph.rs:206-207` |
| HNSW index | `DEFINE INDEX x ON t FIELDS embedding HNSW DIMENSION N DIST COSINE` | `surrealdb/language-tests/.../search-rrf.surql:50` |
| Vector KNN | `SELECT id, vector::distance::knn() AS d FROM t WHERE embedding <\|k,ef\|> $vec` | `surrealdb/language-tests/.../search-rrf.surql:56`; `hadith/src/quran/hadith_refs.rs` |
| BM25 index | `DEFINE INDEX x ON t FIELDS col FULLTEXT ANALYZER a BM25(1.2, 0.75)` | `surrealdb/language-tests/.../7013_search_score_inside_function.surql:42-45` |
| BM25 query | `SELECT ... FROM t WHERE col @N@ "term"` | same as above |
| RRF fusion | `search::rrf([$a, $b, $c], fetch, 60)` — branches may span tables, must share `id` | `surrealdb/core/src/fnc/search.rs:136-142`; `hadith/src/quran/search.rs:46-56` |
| Array difference | `array::difference(a, b)` — **not** `a - b` | `surrealdb/core/benches/functions.rs:15` |
| Jaccard | **Not native** — compute in Rust | scan confirmed |
| `math::exp` | **Does not exist** — compute in Rust | `surrealdb/core/src/fnc/math.rs` |
| `time::diff` | **Does not exist** — use `(time::now() - created_at) / 1d` or Rust | `surrealdb/core/src/fnc/time.rs` |
| Backfill migrations | `DEFINE FIELD IF NOT EXISTS` + later `UPDATE t SET field = ...` | `hadith/src/db.rs:54-55` |

## D. Resolved design questions

1. **Memory scoping — project-scoped.** `project: string` is added to `hifz`, `semantic_hifz`, `procedural_hifz`, `hifz_core`, `run`, `mem_link`, `entity`. All search and context queries filter on `project`. Existing rows backfill from `session_ids[0].project`.
2. **Embedding input — richer text.** We embed `title + "\n" + content + "\nconcepts: " + concepts + "\nfiles: " + files`. Ablations can compare against title+content only.
3. **Evolution scope — curated tiers only.** Evolution runs on `hifz`, `semantic_hifz`, `procedural_hifz`. Observations stay ephemeral — evolving them would be cost-prohibitive with no retrieval benefit, and it matches A-MEM's intent (curated notes, not raw traces).

## E. LLM-as-reranker: the debate

hifz exposes both a fastembed cross-encoder path and an LLM listwise path behind `memory-bench --rerank=<spec>`. Which is "correct" is contested in the retrieval community. This section captures the argument in both directions so future-us doesn't re-litigate it, and so the null result from our bge-base run (Recall@5 0.944 → 0.900) is read in context.

### The case against LLM reranking (the "fundamentally wrong" position, honestly stated)

1. **Training-objective mismatch.** Cross-encoders like bge are trained *specifically* on (query, doc, relevance) triples with a ranking loss. LLMs are trained on next-token prediction. Using an LLM to produce a ranking is asking it to do something its objective function never rewarded directly.
2. **Latency & cost at scale.** 10–100× slower and more expensive per rerank. For a production memory system with thousands of queries/day, this compounds badly.
3. **Position bias.** LLMs have a documented tendency to over-rank items that appear early or late in a listwise prompt. [Liu et al. 2024 "Lost in the Middle"](https://arxiv.org/abs/2307.03172) is the canonical citation.
4. **Non-determinism.** Even at temperature=0, identical inputs can produce different outputs due to floating-point non-associativity in batched kernels. Rerankers are deterministic.
5. **JSON fragility.** You have to parse structured output from a generative model. Our own code in [src/llm_rerank.rs](../src/llm_rerank.rs) has a whole validation path precisely because the LLM can return malformed output.
6. **The real answer is domain fine-tuning.** If a public cross-encoder is weak on your domain (which bge-base was, for us), the principled fix is to fine-tune a small cross-encoder on domain data — not to throw a 7B generative model at it. The LLM route is often a shortcut that masks the missing fine-tuning data.

### The case for LLM reranking (in narrow cases)

1. **Zero-shot on unseen domains.** Papers like [RankGPT](https://arxiv.org/abs/2304.09542) and RankVicuna show generalist LLMs can be competitive or better than fine-tuned rerankers on out-of-distribution data — which is exactly the hifz situation (no training data, domain-specific text).
2. **World knowledge.** LLMs know "ORM ↔ sqlx/diesel/Postgres" from pretraining; bge-base does not. For short, technical probes this matters.
3. **No data to fine-tune.** If you don't have labeled pairs, the choice is "public cross-encoder (domain-mismatched) or LLM (zero-shot)", and LLM can win.

### Where this leaves hifz

We expose both paths and default to neither. The bench is the arbiter; conclusions only after data. Production should not route through either until eval shows a clear win that survives on a real (non-synthetic) corpus.

Long-term correct path: if a public cross-encoder is ever shown to be the bottleneck, the principled fix is fine-tuning on hifz-style (query, memory, relevance) pairs — not a permanent LLM hop.
