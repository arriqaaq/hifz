# hifz Memory Architecture — Research

This doc captures the *why* behind hifz's memory design: prior art, tradeoffs, the evaluation plan, and the verified SurrealDB syntax we rely on. It is the companion to [`../architecture/memory.md`](../architecture/memory.md), which describes the *how*. Both documents are updated at the end of each implementation phase.

## Goal

Turn hifz from log-with-search into a system whose injected context the LLM consistently reaches for. Bias: deterministic / local-first. **LLM only runs when `HIFZ_LLM_EVOLVE=true`** and only for A-MEM–style Memory Evolution (on new-memory write, select neighbours and rewrite their metadata). All retrieval, linking, and injection work without an LLM.

## Known gaps in the current implementation (baseline)

1. Memories (`hifz` table) have no embeddings — `search_memories` is BM25-only.
2. `access_count` is dead — initialised at consolidation, never incremented on retrieval, never read for ranking.
3. Ranking ignores recency — no `created_at` / `last_accessed_at` in sort.
4. Context injection is a blind top-10-by-`strength` dump regardless of session topic.
5. "Graph-expanded search" is advertised in the recall skill doc but `RELATE` appears nowhere in `src/`.
6. No core / working-memory tier — identity, active goals, invariants drift out on compaction.

## A. Prior art synthesis

| System | What we borrow | What we don't |
|---|---|---|
| **A-MEM** ([arxiv:2502.12110](https://arxiv.org/pdf/2502.12110)) | Memory Evolution: on new-memory write, LLM mutates *neighbours'* tags/context/links. Zettelkasten-style graph. | Always-on LLM in the write path — we make it opt-in. |
| **Mem0** ([arxiv:2504.19413](https://arxiv.org/pdf/2504.19413)) | BM25 + vector + graph RRF; recency decay `exp(-age/30)`; access reinforcement `+0.1 per retrieval, cap +2.0`. | Their proprietary orchestrator; we use SurrealDB RRF. |
| **MemGPT** ([arxiv:2310.08560](https://arxiv.org/abs/2310.08560)) | "Core memory" tier always prepended. | OS-style paging between tiers — hifz doesn't need it given SurrealDB is local. |
| **MIRIX** ([Agent-Memory-Paper-List](https://github.com/Shichun-Liu/Agent-Memory-Paper-List)) | Typed memory modules (semantic / procedural / episodic) — hifz already has `semantic_hifz`, `procedural_hifz`; we add `episode`. | Full multimodal / resource / knowledge-vault tiers. |
| **Position: Episodic Memory is Missing** ([arxiv:2502.06975](https://arxiv.org/pdf/2502.06975)) | Task-scoped episode as the unit of replay. | Separate episodic index per agent persona. |
| **MemoryBank** | Ebbinghaus forgetting curve — informs the `exp(-age/30)` default. | |

## B. Design tradeoffs

- **Deterministic graph vs LLM-proposed links.** Base graph is KNN + set-overlap. LLM evolution layers on top as opt-in. Retrieval doesn't require creativity; it must be fast and free. The LLM earns its cost rewriting neighbour metadata, not deciding edges.
- **Per-via edge tables vs single `mem_link` with `via` field.** Single table for join simplicity; `UNIQUE` on `RELATE` only covers `(in, out)`, so per-`via` uniqueness is enforced in Rust. Four-table alternative would give native uniqueness at 4× schema cost.
- **Rust-side scoring.** Forced by the fact that SurrealDB has neither `math::exp` nor `time::diff`. Side benefit: scoring experiments don't require schema migrations.
- **Async evolution.** Write path commits deterministically, evolution runs in a background queue after the dedup window. Write latency stays local. Evolution output is JSON-capped so misbehaving prompts can't corrupt the graph.
- **`RELATE UNIQUE` limitation** (`(in, out)` only) is the biggest footgun — documented, with Rust dedup as the mitigation.
- **Threshold values are initial guesses:** MMR 0.85, KNN cosine-distance 0.25 (`via='embedding'`), Jaccard 0.3 (concept/file). Phase 6 eval harness tunes them.

## B1. Verified behaviours from evals

Pre-fix baseline (Phases 1-6 shipped, `memory-bench --full`, 30 memories / 90 probes):

| Metric | Value | Note |
|---|---|---|
| Recall@5 | 0.856 | |
| Recall@10 | 0.856 | **identical to Recall@5** — smoking gun for a truncation bug |
| MRR | 0.791 | |
| Injection@top | 0.833 | |

Diagnosis (Phase 7a): `diversify_by_session` at [src/search.rs:425-437](../src/search.rs#L425-L437) keyed every memory under the literal string `"memory"`, collapsing the entire memory pool into a single 3-result bucket. Fixed by keying memories by their own record id. Regression test in [src/search.rs::tests::diversification_does_not_collapse_memories](../src/search.rs). Post-fix numbers will be recorded below after the next bench run.

Ablation flags available for future evals (Phase 7c): `memory-bench --ablate=vector,recency,graph,diversify` — each flag disables the named stage so future regressions can be attributed to the right component.

**Post-Phase-7a numbers** (bug-fix run): Recall@5 = 0.922, Recall@10 = 0.967, Recall@20 = 1.000, MRR = 0.815, Injection@top = 0.933. Recall@20 saturating means retrieval is complete — every oracle is reachable; remaining misses are *ranking-tier* issues, not retrieval holes.

**Phase 8 — diagnostics added, multi-oracle scaffolding removed:** Initial hypothesis was that the miss list after 7a was partly fixture noise (probes matching multiple memories). We added `alt_oracles_for` as an explicit whitelist of probes → multiple accepted oracle titles, and the competitor diagnostic to inspect misses. The diagnostic stayed; the whitelist was removed after Phase 9.1 (see below) because sharper RRF resolved the ambiguity cases without needing an escape hatch, and carrying a whitelist without evidence it still pulls its weight is curve-fitting.

Kept from Phase 8:
1. Per-miss diagnostic printing the memories ranking above the oracle with their scores.
2. `--preprocess=strip-project` flag (Phase 8.5) — useful for one-off hypothesis tests.

**Phase 8.5 — project-token dilution hypothesis:** Added `--preprocess=strip-project` bench flag (strip `"hifz"` and project name from probes). Confirmed that 2 of 7 residual misses were token dilution (the oracle recovered when the `"hifz"` token was removed). But stripping also *hurt* the `tokio runtime config in hifz` probe by one rank, so preprocessing is not shipped to production — it's a diagnostic only.

**Phase 9.1 — RRF k sweep (the principled dilution fix):**

| k | Recall@5 | Recall@10 | MRR | Misses |
|---|---|---|---|---|
| 60 (old default) | 0.922 | 0.967 | 0.830 | 7 |
| 20 | 0.922 | 0.967 | 0.830 | 7 (identical) |
| **10 (new default)** | **0.944** | **0.978** | **0.845** | **5** |
| 5  | 0.956 | 0.978 | 0.871 | 4 |

k=20 was a no-op — RRF is insensitive to modest k changes when every branch uses the same constant. k=10 crossed the threshold where cross-branch ordering actually shifts in favour of the true top-1. **Default lowered from 60 to 10 in [src/search.rs](../src/search.rs) `SearchConfig::default()`.** The literature's k=60 is TREC-scale robust; for personal-memory corpora at 30-scale the sharper curve matches the data. k=5 was strictly better on the fixture but held back to avoid overfitting small-corpus tuning. Override via `SearchConfig::rrf_k` or `memory-bench --rrf-k=N`.

## C. Evaluation plan (lives in `crates/eval/README.md`)

- **Metric 1 — Retrieval:** 200 (memory, probe) pairs. Recall@5, Recall@10, MRR. Ablations: `--no-vector`, `--no-recency`, `--no-access`, `--no-graph`.
- **Metric 2 — Injection hit rate:** held-out prompts; does `/hifz/context` include the oracle memory in the first N tokens?
- **Metric 3 — Evolution quality** (flag on): synthetic superseding pairs; does evolution correctly set `is_latest` and `supersedes` within one write?
- **Targets:** ≥15% absolute Recall@5 lift over the current BM25-only baseline after Phase 1; further ≥5% after Phase 3 graph expansion; Phase 5 must improve Metric 3 without regressing Metrics 1–2.
- **External benchmark aspiration:** run LongMemEval once the internal harness is stable.

## D. Verified SurrealDB syntax reference

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

## E. Resolved design questions

1. **Memory scoping — project-scoped.** `project: string` is added to `hifz`, `semantic_hifz`, `procedural_hifz`, `hifz_core`, `episode`, `mem_link`, `entity`. All search and context queries filter on `project`. Existing rows backfill from `session_ids[0].project`.
2. **Embedding input — richer text.** We embed `title + "\n" + content + "\nconcepts: " + concepts + "\nfiles: " + files`. Phase 6 ablations compare against title+content only.
3. **Evolution scope — curated tiers only.** Evolution runs on `hifz`, `semantic_hifz`, `procedural_hifz`. Observations stay ephemeral — evolving them would be cost-prohibitive with no retrieval benefit, and it matches A-MEM's intent (curated notes, not raw traces).
