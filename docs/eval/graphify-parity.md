# hifz vs graphify — capability parity

Honest "where we match / where we don't", from graphify's verbatim README/site
claims (`/Users/kfarhan/workspace/projects/graphify`) measured against hifz
reality. The one *measurable* claim (token reduction) is reproduced as a
runnable, comparable benchmark: `cargo run --release --bin parity-bench`.

## What graphify is

A **stateless, portable code-graph snapshot tool** (Python/NetworkX/JSON):
`/graphify` parses a project with tree-sitter into a knowledge graph, runs
community detection, and exposes it as MCP query tools + exports
(HTML/SVG/Obsidian/Neo4j/Mermaid). The graph is frozen at extraction time;
there is no persistence across sessions, no embeddings, no commit grounding.

## Matrix

| Capability | graphify | hifz | Verdict |
|---|---|---|---|
| Code graph via tree-sitter | ✅ 31 languages | ✅ ~9 (rs, py, ts/js, go, java, c/cpp) | **graphify** (breadth) |
| NL code search | IDF + graph BFS, **no embeddings** | hybrid **vector + BM25 + RRF** | **hifz** (semantic) |
| Persistent cross-session memory | ❌ none (frozen snapshot) | ✅ observations/decisions/lessons | **hifz** |
| Commit grounding (rank by what shipped) | ❌ | ✅ `ground.rs` + git post-commit hook | **hifz** (unique) |
| Temporal reasoning / recency decay | ❌ | ✅ Ebbinghaus `exp(-age/30)` + access boost | **hifz** |
| Knowledge-graph expansion at query | ✅ BFS/DFS, hub-aware | ✅ 2-hop typed edges, dampened | parity |
| Community detection | ✅ Leiden/Louvain | ✅ | parity |
| MCP server | ✅ query/node/neighbors/path/PR tools | ✅ recall/search/code/trace/neighbors/… | parity |
| PR / worktree awareness | ✅ triage, impact | partial (commit-linked) | **graphify** |
| Multi-modal (PDF/image/video/audio) | ✅ | ❌ (atlas ingests docs only) | **graphify** |
| Export formats (HTML/SVG/Obsidian/Neo4j) | ✅ rich | ❌ (JSON export only) | **graphify** |
| Portable, DB-free, offline snapshot | ✅ commit `graphify-out/` | ❌ (SurrealKV daemon) | **graphify** |
| IDE/agent breadth | ✅ 15+ assistants | Claude Code | **graphify** |
| Token reduction vs grepping | ✅ 71.5x (52-file multimodal corpus) | measured by `parity-bench` | comparable |

## The measurable claim, made comparable

graphify: `reduction_ratio = corpus_tokens / avg_query_tokens`, with
`tokens ≈ max(1, chars/4)` (graphify/benchmark.py). `parity-bench` applies the
**identical** estimator and ratio to hifz's `code_search` output vs the indexed
corpus, so the two numbers are directly comparable. graphify's headline 71.5x
is on a 52-file *multimodal* corpus (code+papers+images); their own small-code
corpora compress far less (~1–5x) — `parity-bench` gates at the ≥5x
small-corpus floor and additionally asserts every claimed language still
indexes (regression guard).

## Honest summary

hifz and graphify are **complementary, not competitors**. graphify is a
broad-language, portable, multimodal *snapshot*; hifz is a *persistent,
commit-grounded memory* with semantic retrieval. hifz should not chase
graphify's breadth/export/multimodal surface; its defensible axis is
commit-grounding + cross-session memory, which graphify explicitly does not
have. Where hifz is genuinely behind and it matters: **language breadth** —
tracked by `parity-bench`'s language-coverage gate.

> **See also:** [code-retrieval.md](code-retrieval.md) — a single-corpus
> token-efficiency benchmark (atlas vs code-search vs the grep baseline an
> agent uses with no tool). Fullerenes is the TypeScript analogue of graphify;
> a Burhan debate established an unbiased hifz-vs-Fullerenes head-to-head is a
> category error, so that comparison is intentionally *not* run.
