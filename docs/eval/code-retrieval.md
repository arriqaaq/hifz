# code_search retrieval-correctness diagnosis (not token theater)

**Question:** does hifz `search_code` actually find the right function, and when
it doesn't, **why** — and is that fixable? If retrieval is wrong, the agent's
whole reasoning is wrong; saving tokens on a wrong retrieval is *negative*
value. So correctness is the headline; token cost is informational only.

Runnable: `make code-retrieval-bench` (or `cargo run --release --bin
code-retrieval-bench -- --root .`). Diagnose-only — no retrieval/library code
is changed; the localized fix is the documented next step.

## Method

- **Corpus**: the chosen tree (default: whole hifz repo, gitignore-filtered).
- **Oracle**: docstring→code (CodeSearchNet style), copied verbatim from
  `benchmark/corpus_code_bench.rs` (untouched; this bench is additive). The
  **paraphrase** style (identifiers deterministically stripped) is the semantic
  test the gate lives on; **raw** is a lexical-overlap-biased sanity row.
- **Correctness metric**: per paraphrase probe, `search_code(limit=K=10)` →
  - **file-hit@K**: any returned result from the oracle *file* (exact
    repo-relative path; no basename fallback — paths are identical `f.rel`
    form, the fallback only adds cross-crate collisions). This is the
    grep-comparable "did the agent get the right file into context".
  - **chunk-rank@K**: stricter — a returned chunk's line span overlaps the
    symbol span. Reported alongside file-hit so neither flatters.
- **grep**: token-cost baseline only (generous: distinctive len≥4 terms). Its
  file-presence "recall" is a different granularity — reported, never gated.

## Localization — every paraphrase probe → exactly one bucket

- **HIT**: file-hit@K (correct retrieval).
  - sub-count **T2 (chunk-granularity artifact)**: file-hit but chunk-rank
    miss. A measurement control quantifying how much a chunk-oracle *understates*
    correctness — reported separately, never used to discount a real miss,
    excluded from `genuine_fail`.
- **T3 (unindexed-symbol)**: the symbol has no indexed `code_chunk`
  (`splitter.rs:74` `min_chars=250` non-first-chunk drop / indexing gap).
  Genuine but infrastructural.
- **T1a (recoverable)**: file-miss@K, but the oracle file *is* present in a
  WIDE=200 fused pool **or** BM25-wide → a signal surfaces it; the
  ≤`limit`-per-branch pool + `max(vec, bm25/4)` fusion + no rerank
  (`src/code/search.rs:87-160`) buried it before top-K. **Fixable.**
- **T1b (representation failure)**: file-miss@K and absent even from WIDE fused
  *and* BM25-wide → neither signal surfaces it in 200 → embeddings / chunking /
  index. **Deep.**

Headline: `genuine_fail = (T1a + T1b + T3) / N`, and
`recoverable_share = T1a / (T1a + T1b + T3)`.

## Gate (correctness only)

- **SKIP** (exit 2): <25 usable / <25 paraphrasable / 0 symbols indexed.
- **PASS** (exit 0) iff paraphrase **file-hit recall ≥ 0.50** — the project's
  own pre-registered floor (`benchmark/corpus_code_bench.rs:372`). Gate on
  *correctness*, nothing else.
- **FAIL** otherwise. Taxonomy, chunk-rank, tokens: **reported, not gated**.

## Tokens — informational only

`code_search` avg tokens are reported **only on the HIT set** beside the grep
file-cost baseline, in a clearly-labelled secondary section. They are **not the
headline, not a ratio claim, not gated**, and explicitly null-value on the
`genuine_fail` fraction (token savings on a wrong retrieval are negative value).
Kept purely for information.

## Results — whole hifz repo (`--root .`)

_Filled by the run; artifact `benchmark/data/code_retrieval_results.json`._

```
PARAPHRASE (n=…)
  genuine_fail = …%  (T1a_recoverable=… T1b_representation=… T3_unindexed=…)
  HIT (file-hit@10) = … %   of which chunk-granularity artifact (T2) = … %
  of genuine failures, …% are RECOVERABLE
raw-doc sanity: file-hit@10=…  chunk-rank@10=…
tokens (secondary): code_search avg(HIT)=…  grep avg=…  grep file-hit=…%
GATE: PASS|FAIL|SKIP
```

**Reading (filled after the run):** if **T1a ≫ T1b**, the dominant cause is the
`limit`-bounded candidate pool + crude `max(vec,bm25/4)` fusion + no rerank in
`search_code` — *recoverable* by retrieving N≫K per branch then RRF-fuse +
rerank (mirroring what memory `search.rs` already does and `search_code` does
not). If **T1b ≫ T1a**, the cause is representation (embeddings/chunking) — a
deeper fix. The number decides; this doc is updated with the measured verdict,
not a prejudged one.

## Honest notes

- Earlier honesty trail (kept): a prior gate condition `code_search recall ≥
  grep recall` was removed — not flipped green — because grep's file-presence
  recall is a different granularity than chunk-rank and (queries deriving from
  their own docstring) is unconditionally ≈1.0, making it a broken,
  unsatisfiable condition rather than a signal. grep is the token-cost
  denominator only.
- file-hit is the fair grep-comparable correctness metric but *looser* than
  chunk-rank; both reported. The headline is the genuine (T1a+T1b+T3) failure
  rate — not the friendlier number.
- Single project (hifz); `--root` makes it repeatable. Diagnose-only: no
  retrieval behavior changed here.

See also: [graphify-parity.md](graphify-parity.md).
