# hifz Memory Architecture — Reference

The *how*. See [`../research/memory-architecture.md`](../research/memory-architecture.md) for the *why*.

This document is the ground truth for hifz's memory model. It is updated at the end of each implementation phase, so sections may describe near-future state — any diagram or table that is ahead of the code is called out with **(planned)**.

## Phase status

| Phase | Status |
|---|---|
| 0 — Docs (this file + research doc) | shipped |
| 1a — Memories embedded, project-scoped; reindex CLI | shipped |
| 1b — Rust-side strength·recency·access scoring; access bump on read | shipped |
| 1c — Query-aware injection + MMR-lite (mem_type + first concept) | shipped |
| 2 — Core / working memory | shipped |
| 3 — Graph linking (`edge`) | shipped |
| 4 — Entities + runs (auto-run in observe pipeline) | shipped |
| 5 — Memory Evolution (opt-in LLM, `HIFZ_LLM_EVOLVE=true`) | shipped |
| 6 — Eval harness (`memory-bench`) | shipped |
| 7a — `diversify_by_session` memory cap fix + regression test | shipped |
| 7b — `memory-bench` per-miss diagnostics (rank in pool / not-in-pool) | shipped |
| 7c — `SearchConfig` ablation knobs + `--ablate=` CLI flag | shipped |
| 8 — multi-oracle probes + competitor diagnostic in bench | shipped |
| 8.5 — `--preprocess=strip-project` diagnostic flag (bench only) | shipped |
| 9.1 — `SearchConfig::rrf_k` tuning knob, default lowered 60 → 10 | shipped |
| 1c.2 — Cosine-based MMR (successor to MMR-lite) | planned |

---

## 1. Data model

<p align="center"><img src="../img/data-model.svg" alt="hifz data model" width="100%"></p>

### Tier semantics

- **`observation`** — dense, ephemeral, auto-captured by hooks. Has `obs_type` and `metadata` fields. Notable types: `commit_made`, `plan_activated`, `session_summary`. Never evolved.
- **`memory`** — curated, project-scoped long-term knowledge. Embedded + indexed + linked. `pinned=true` marks always-on entries (e.g. active plans). May be evolved.
- **`semantic_memory`** — facts consolidated from sessions (tier 1 consolidation). May be evolved.
- **`procedural_memory`** — workflows consolidated from observations (tier 3 consolidation). May be evolved.
- **`run`** — task-scoped trajectory (Phase 4).
- **`core_memory`** — per-project singleton: identity, goals, invariants, watchlist (Phase 2).
- **`edge`** — typed graph edges between memories (Phase 3).
- **`entity`** — typed named things mentioned by observations and memories (Phase 4).

---

## 2. Write pipeline (observation → memory)

<p align="center"><img src="../img/write-path.svg" alt="hifz write path" width="100%"></p>

---

## 3. Read / injection pipeline

<p align="center"><img src="../img/read-path.svg" alt="hifz read path" width="100%"></p>

After ranking, results are MMR-diversified (cos sim ≤ 0.85), fit to a token budget (1500/2048), and emitted as `# Core` / `# Saved memories` / `# Recent observations` sections to the Claude Code `additionalContext` field. Each surfaced memory's `access_count` is bumped and `last_accessed_at` updated, feeding the access component of the rank formula on subsequent queries.

### Diversification rule

- **Observations** are capped at 3 per `session_id` so a single noisy session can't dominate the results.
- **Memories** have no `session_id`; they are keyed by their own record id during diversification so each memory is its own class. The cap is therefore a no-op for memories. (Phase 7a fixed a bug where every memory was bucketed under the literal string `"memory"`, silently truncating the memory pool to 3 results per query.)

### Scoring formula (Rust-side)

```
score = strength
      * exp(-age_days / 30)                           # Ebbinghaus decay
      * (1.0 + 0.1 * min(access_count, 20))           # usage reinforcement, cap +2.0
```

Forced Rust-side because SurrealDB lacks `math::exp` and `time::diff`. Side benefit: tuning does not require schema changes.

### Query construction by trigger

| Hook | Query string |
|---|---|
| `SessionStart` | project name + titles of N most-recent high-importance observations |
| `UserPromptSubmit` | the prompt text |
| `PreCompact` | titles of observations since last compaction |

---

## 4. Graph linking

Base graph is deterministic. Edges are created at write time.

### Link `via` values

| via | Condition | Score |
|---|---|---|
| `embedding` | KNN cosine distance < 0.25 | `1 - distance` |
| `concept` | Jaccard on `concepts` ≥ 0.3 | Jaccard value |
| `file` | Jaccard on `files` ≥ 0.3 | Jaccard value |
| `entity` | shared entity count ≥ 1 | normalised count |
| `semantic` | proposed by evolution (Phase 5, LLM) | LLM-proposed |

### Edge dedup

`RELATE ... UNIQUE` enforces `(in, out)` uniqueness, not `(in, out, via)`. So per-`via` dedup happens Rust-side: before `RELATE`, `SELECT ... WHERE in=$a AND out=$b AND via=$via`; if present, `UPDATE` with `math::max(score, new)`; otherwise `RELATE`.

### 1-hop expansion at read time

Two queries — `SELECT ->edge->node.*` does *not* return edge fields, so edges and neighbour rows are fetched separately and joined in Rust:

```surql
SELECT in, out, score, via FROM edge WHERE in IN $top_ids;

SELECT id, title, content, mem_type, strength, created_at, access_count
FROM memory WHERE id IN $neighbour_ids AND is_latest = true;
```

Neighbours are scored `seed_score * 0.5 * edge.score`, merged with primary candidates, re-ranked with the same Rust-side formula, then MMR-diversified.

---

## 5. Memory Evolution (opt-in)

Gated by `HIFZ_LLM_EVOLVE=true`. Matches A-MEM §*Memory Evolution*: on a new-memory write, the LLM inspects the new note + its KNN/graph neighbours, then proposes *updates to the neighbours*.

<p align="center"><img src="../img/evolve.svg" alt="memory evolution sequence" width="100%"></p>

### LLM output contract

```json
{
  "new_note": { "keywords": [...], "tags": [...], "context": "why-this-matters one-liner" },
  "neighbour_updates": [
    {
      "id": "memory:abc",
      "keywords_add":  [...], "keywords_remove": [...],
      "tags_add":      [...], "tags_remove":     [...],
      "context_rewrite": "…" | null,
      "link_to_new":   { "create": true,  "via": "semantic", "score": 0.0 } | null,
      "supersedes_new": false,
      "superseded_by_new": false
    }
  ]
}
```

### Safety

- Cap `neighbour_updates` to 5 per evolution call.
- JSON-only contract — misbehaving prompts can't corrupt the graph.
- Deterministic path (flag off) is fully functional; Phases 1–4 do not depend on this.

### Triggers

- After write, once the dedup window in `src/dedup.rs` expires.
- During the nightly consolidation tick for memories that skipped evolution.
- Manual: MCP tool `hifz_evolve(memory_id)`.

---

## 6. Phase map

Build order (each block depends on the prior):

1. **Phase 6 — Eval harness** (`memory-bench`). Foundation: nothing else lands without measurement.
2. **Phase 1 — Retrieval quality**. Embedded memories, project scoping, Rust-side rank, query-aware injection.
3. **Phase 2 — Core memory** and **Phase 3 — Graph linking** (parallel; both build on Phase 1).
4. **Phase 4 — Entities + runs**. Builds on Phase 3.
5. **Phase 5 — Evolution (LLM, opt-in)**. Builds on Phase 4.

See the **Phase status** table at the top of this document for the current shipped/planned state of each phase.

## 7. Migrations

Per-phase pattern (verified from `hadith/src/db.rs:54-55` and its `backfill_narrator_hadith_counts`):

1. `DEFINE FIELD IF NOT EXISTS` / `DEFINE INDEX IF NOT EXISTS` at startup (schema init in `src/db.rs` is already idempotent).
2. One-shot backfill via `hifz reindex [--memories|--entities|--links]`.
3. Backfill order: embeddings → entities → links → evolution.
