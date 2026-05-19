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

## Headline metric & localization

The headline is **corrected chunk-hit@K**: a returned chunk overlaps
`[doc_top..end]` — i.e. the agent actually got *this function* (its doc +
body), not merely the right file. `doc_top` is the topmost line of the doc
block above the def (the strict `code_symbol.start_line` excludes the `///` doc
the query is derived from; `codegraph.rs:149-150`). `doc_top` and the
corrected/strict overlap are **unit-tested**.

`genuine_fail = N − chunk_hit`. Every non-hit → exactly one bucket:

- **recoverable**: the right chunk *is* reachable in a WIDE=200 search (or the
  file via BM25-wide) but the ≤`limit`-per-branch pool + `max(vec,bm25/4)`
  fusion + no rerank (`src/code/search.rs:87-160`) buried it before top-K.
  Split into *right-file/wrong-region* vs *wrong-file*. **Fixable.**
- **deep**: not reachable even at WIDE → chunking / embedding / representation.
- **infra (unindexed)**: the symbol has no indexed `code_chunk` over
  `[doc_top..end]` at all (`splitter.rs:74` `min_chars=250` non-first-chunk
  drop). Infrastructural.
- **oracle-correction audit**: count of *correct* hits the doc-excluding strict
  `[start..end]` span would have wrongly missed — reported so the
  `[doc_top..end]` leniency is auditable, never to flatter.

`file-hit@K` (any returned result from the oracle file, exact path) is also
reported as a *looser, grep-comparable* number — explicitly **not** the
headline.

## Gate (correctness only)

- **SKIP** (exit 2): <25 usable / <25 paraphrasable / 0 symbols indexed.
- **PASS** (exit 0) iff paraphrase **corrected chunk-hit recall ≥ 0.50** — the
  project's own pre-registered floor (`benchmark/corpus_code_bench.rs:372`).
- **FAIL** otherwise. file-hit, the taxonomy, and tokens: **reported, not
  gated**.

## Tokens — informational only

`code_search` avg tokens are reported **only on the correct (chunk-hit) set**
beside the grep file-cost baseline, in a clearly-labelled section. They are
**not the headline, not a ratio claim, not gated**, and explicitly null-value
on the `genuine_fail` fraction (token savings on a wrong retrieval are negative
value). Kept purely for information.

## Results — whole hifz repo (`--root .`, K=10, WIDE=200)

`index_repo`: 135 files, 1595 chunks, 1536 symbols · 447 usable documented
probes (390 paraphrasable). Artifact:
`benchmark/data/code_retrieval_results.json`. `doc_top` and the
corrected/strict overlap are unit-tested (8/8, `cargo test --bin
code-retrieval-bench`).

```
PARAPHRASE (semantic, identifier-stripped, n=390)
  HEADLINE chunk-hit@10 (agent actually got the function) = 380 [97.4%]
  genuine_fail = 10 [2.6%]   recoverable=8 [2.1%]  deep=2 [0.5%]  infra=0 [0.0%]
    ├─ right-file/wrong-region: recoverable=2 deep=2
    └─ wrong-file:              recoverable=6 deep=0 unindexed=0
  file-hit@10 = 384 [98.5%]  (looser, grep-comparable, NOT the headline)
  ORACLE-CORRECTION AUDIT: 104 [26.7%] of correct hits would have been WRONGLY
    scored misses by the doc-excluding strict [start..end] span
raw-doc sanity: file-hit@10 = 0.991  chunk-hit@10 = 0.982 (n=447)
tokens (informational): code_search avg on CORRECT set = 1940  |  grep avg =
  240556  |  grep file-hit = 99.7%
GATE: PASS
```

**The verdict (both biases checked).** hifz `search_code` retrieves the right
function **97.4%** of the time on hard semantic (identifier-stripped) queries
across the whole repo. The earlier alarming "~0.66 R@5 / ~28% gap" was **~96% a
broken yardstick, not broken retrieval**: `code_symbol.start_line` excludes the
`///` doc comment the query is *derived from* (verified
`codegraph.rs:149-150`), so a correct retrieval landing on the doc+signature
chunk was scored a miss. The corrected oracle spans the function *plus its doc*
(`[doc_top..end]` — what is actually indexed and queried); the 26.7%
**oracle-correction audit** number is reported precisely so this leniency is
auditable, not a flatter, and the correction logic is unit-tested. Residual
*genuine* failure is **2.6%** (10/390): 8 recoverable (the right chunk exists
in a WIDE=200 pool but the `limit`-bounded `max(vec,bm25/4)` no-rerank fusion
in `src/code/search.rs:87-160` buries it before top-10), only 2 deep
representation. The localized, evidence-keyed fix (next iteration, not done
here): retrieve N≫K per branch → RRF-fuse → rerank, mirroring what memory
`search.rs` already does and `search_code` does not.

**Leniency caveat (stated, not hidden):** the corrected oracle counts a
returned chunk that overlaps the doc-comment region (not necessarily the body)
as a hit. This is legitimate — the query *is* the docstring and the doc is
contiguous with / names the function, so an agent landing there has the
function — but it is a deliberate looseness; the strict-span audit (26.7%)
exists so a reviewer can see exactly how much it moves the number.

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

## Experiment — widen+RRF hypothesis: REJECTED (pre-registered, --root .)

The diagnosis localized the ~2% residual as "recoverable" (oracle present in a
WIDE=200 pool but buried by the `≤limit` pool + `max(vec,bm25/4)` + no-rerank
fusion). A pre-registered, in-benchmark A/B tested that fix **without touching
production `search_code`** (A1/A2 are bench-local raw SurrealQL; the RRF SQL is
unit-smoke-tested):

```
arm           chunk-hit@10   Δ vs A0   recov→hit   raw chunk-hit   p90 ms
A0 control    385 [97.7%]    --        0/8         444/451         13.7
A1 widen+max  385 [97.7%]    +0.0pp    0/8         443/451         15.6
A2 widen+RRF  384 [97.5%]    −0.3pp    0/8         444/451         35.0
GATE: C1 FAIL(+0.0pp<+1.0) · C2 FAIL(0/8<50%) · C3 PASS · C4 PASS → REJECT
```

**Verdict: REJECT — `search_code` is NOT modified.** Widening the pool to 200
converted **0 of 8** recoverable misses; RRF was slightly *worse* and 2.7×
slower. Internal validity holds (A1/A2 reproduce A0 on easy raw-doc queries;
10/10 unit + SQL-smoke tests pass) — the null is real.

**Why the hypothesis was wrong (honest correction).** "Recoverable" only meant
the oracle appeared *somewhere* in a 200-wide pool — not that a wider pool or
better fusion lifts it into the top-10. Those chunks are buried by **score**
(both vector and BM25 rank them ~30+), not by pool truncation; RRF of two
poor ranks is still a poor fused rank. The ~2% residual is **not** a
candidate-pool or fusion-arithmetic problem — it is a deeper representation
limit or near the irreducible floor for this oracle/corpus. The experiment did
its job: it falsified a plausible fix *before* any production change. No
further fix is pursued (closing it = the anti-goalpost-moving discipline).

See also: [graphify-parity.md](graphify-parity.md).
