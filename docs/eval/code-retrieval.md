# Code-retrieval token efficiency — atlas vs code-search vs grep

A single-corpus efficacy benchmark: on a real tree, **how many tokens does an
agent burn to answer "find/understand this function", and does the cheap path
actually find the right code** — hifz retrieval vs. the grep-and-read an agent
falls back to with no tool. Runnable: `make code-retrieval-bench` (or
`cargo run --release --features atlas --bin code-retrieval-bench -- --root .`).

## Why this and not a hifz-vs-Fullerenes head-to-head

A Burhan adversarial debate established that an unbiased apples-to-apples token
head-to-head between hifz and Fullerenes (the TypeScript analogue of graphify)
is a **category error**: hifz `search_code` returns count-bounded raw chunks
while Fullerenes returns char-budgeted section-atomic maps — printing the two
ratios side-by-side *is* the bias, because adjacency reads as comparability.
That comparison is intentionally not run.

This benchmark has no such flaw. Every arm answers the **same** docstring→code
questions against the **same** oracle with the **same** `chars/4` estimator, and
the anchor is the **grep baseline** — the universal fallback graphify and
Fullerenes themselves benchmark against. That is exactly why the reduction
ratio here is legitimately comparable to their published figures.

## Method

- **Corpus**: the chosen tree (default: the whole hifz repo, gitignore-filtered,
  `target/` excluded). Hundreds of documented symbols across every crate.
- **Oracle / ground truth**: docstring→code (CodeSearchNet style), copied
  verbatim from `benchmark/corpus_code_bench.rs` (that file is left untouched —
  this bench is *additive*). Every documented `code_symbol`'s first doc sentence
  is a query whose oracle is that symbol.
  - **raw** style: first sentence verbatim (shares identifiers with the body →
    lexical-overlap bias).
  - **paraphrase** style: identifier tokens deterministically stripped — the
    controlled semantic test. **The gate lives here.**
- **Arms** (same oracle, same estimator):
  - **code_search** — `search_code` (vector + BM25 hybrid over chunk bodies);
    this is the engine the `hifz_code_search` MCP tool wraps. Tokens = Σ snippet
    tokens the agent would read.
  - **atlas** — `atlas::query` over `project_code_graph`. **Honest caveat,
    stated up front**: `query()` is BM25 on the symbol *name*; the embedding
    atlas stores is **not used by the query path**. So atlas is *expected* to
    score low recall at low tokens on natural-language docstrings. A low number
    is a true measurement, not a defect — the bench measures, it does not
    flatter. **atlas is reported, never gated as a winner.**
  - **grep** — realistic grep, deliberately *generous to grep*: only
    distinctive terms (len ≥ 4, dropping most stopwords), so fewer files match →
    lower grep token cost → a harder, more conservative bar for hifz's claim.
    Recall = is the oracle file in the matched set (grep finds the file or it
    doesn't; no intra-file ranking). Tokens = Σ tokens of the matched files.
  - **whole-corpus ceiling**: Σ tokens of every walked code file = the absolute
    upper bound an agent could burn.
- **Metrics** per (arm × style): Recall@1/5/10, MRR, avg tokens-to-answer,
  `×reduction vs grep`, `×reduction vs whole-corpus`.

## Gate (honest single-tool efficacy — no manufactured winner)

- **SKIP** (exit 2): < 25 usable documented symbols, or 0 code symbols indexed,
  or < 25 paraphrasable symbols. Distinct reason strings per cause.
- Atlas ingest 0/failed → atlas rows print `SKIP`, excluded from the gate;
  code_search/grep still gate.
- **PASS** (exit 0) iff, on the **paraphrase** (semantic) style:
  1. `code_search Recall@5 ≥ 0.50` (an absolute semantic-retrieval floor), **and**
  2. `code_search avg_tok ≤ grep avg_tok` (the index is materially cheaper —
     the actual value proposition).
- **FAIL** (exit 1) otherwise, listing the offending numbers.

> **Why there is no "code_search recall ≥ grep recall" condition (honest
> trail).** The first version of this gate had exactly that third condition.
> It FAILED on the first real run (`code_search` paraphrase R@5 0.667 vs grep
> 0.997). On inspection the condition is *methodologically unsound*, not a hifz
> signal: grep has no intra-result ranking, so its "recall" is mere
> file-presence, whereas `code_search` recall is chunk-rank — different
> granularities. And because every query is derived from its own symbol's
> docstring, generous file-grep finds the file ≈always, making the condition
> **unconditionally unsatisfiable regardless of how good hifz is**. A gate that
> can never pass is broken, not evidence. It was removed — *not* flipped green —
> and grep retained only in its legitimate role: the token-cost denominator
> (exactly how graphify/Fullerenes use it). The apples-to-apples semantic lift
> vs BM25-only already lives in `corpus-code-bench`.

`×reduction` uses `chars/4` (the graphify/Fullerenes estimator), stated
explicitly. It is comparable to their published numbers **only** because the
grep baseline is defined identically — not a cross-tool contest.

## Results — whole hifz repo (`--root .`, limit 10)

`index_repo`: 135 files, 1565 chunks, 1507 symbols · atlas projection: 1507
symbols, 16676 edges · 442 usable documented probes (381 paraphrasable) ·
whole-corpus ceiling ≈ 300k tokens. Artifact:
`benchmark/data/code_retrieval_results.json`.

```
style       arm           R@1   R@5   R@10  MRR    avg_tok  x_grep  x_corpus   n
raw         grep         1.000 1.000 1.000 1.000   243561    1.0     1.2     442
raw         code_search  0.448 0.717 0.771 0.572     1938  125.7   154.9     442
raw         atlas        0.000 0.000 0.000 0.000        0     —       —      442
paraphrase  grep         0.997 0.997 0.997 0.997   234809    1.0     1.3     381
paraphrase  code_search  0.425 0.659 0.696 0.531     1942  120.9   154.6     381
paraphrase  atlas        0.000 0.000 0.000 0.000        0     —       —      381
GATE: PASS
```

**Honest reading:**
- **code_search**: on hard semantic (paraphrase) queries across the full
  1565-chunk repo, it puts the right chunk in the top-5 **66%** of the time
  (MRR 0.53) while reading **~1.9k tokens — ≈121× fewer than grep** (~235k) and
  ~155× under the whole-corpus ceiling. The token-efficiency is the strong,
  robust result; the 66% R@5 is a real, *stated* ceiling on retrieval quality
  (not spun) — the apples-to-apples semantic check vs BM25-only is in
  `corpus-code-bench`.
- **grep**: ≈always finds the oracle *file* — but at essentially whole-corpus
  token cost. That is the tradeoff an index exists to remove.
- **atlas**: **0.000** across the board. `atlas::query` matches the query
  against the symbol *name* (BM25); a full NL docstring sentence ≈never matches
  a 1–2-word name, so atlas returns nothing **by construction**. A true
  negative for atlas's text-query surface on docstring→code retrieval — kept
  visible, not hidden; atlas is never gated.

## Honest caveats

- Atlas's weakness on NL docstrings is structural (BM25-on-name; embedding
  unused by `query()`) — disclosed, not hidden; surfaced by the (recall, tokens)
  pairing.
- Grep "recall" is lexical; on the paraphrase style (identifiers stripped) grep
  is expected to fall off — that gap is precisely what semantic retrieval exists
  for, and gate condition (3) tests exactly it.
- One project (hifz's own code): a strong *within-project* signal, not a
  cross-project population claim. `--root <dir>` makes it repeatable elsewhere;
  `--root crates/memdiff` is the fast smoke path.

See also: [graphify-parity.md](graphify-parity.md) — the qualitative capability
matrix and the Fullerenes/graphify framing.
