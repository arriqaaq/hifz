# Implementation Progress — commit-grounding (scratch, not for commit)

Design source of truth: `~/.claude/plans/today-if-i-commit-groovy-deer.md`.
(Recreated — prior copy was removed; unrelated pre-existing `plan.md` untouched.)

## Phase 1 — watermark + git-hook adapter: SHIPPED & VERIFIED
1 schema `memory.last_committed_at`; 2 watermark write + clear forget_after on
synchronous commit path (reverts excluded); 3 deleted dead K1
`observation.project` subquery; 4 recency exemption on forget.rs ×2 +
consolidate.rs ×2 (`COMMIT_PROTECT_DAYS=90`); 5 search_memories_with_config
→ pub(crate) (matcher swap = deferred ship-gate); 6 `hifz hook
install|uninstall|status|doctor|ingest` + src/githook.rs. Built clean,
restarted, verified live (grounding strengthened, hooks round-trip, no
regression). Saved to hifz `ship_report` memory.

## Phase 2 — caveat remediation (approved plan, bias-checked)
- [x] Fix 1 hot-file dampening: `HOT_FILE_MAX=20` + async
  `discriminating_files()` in src/ground.rs; filters confirm set
  (on_commit_observation + cfg(code) binds) and contradict set
  (on_commit_signal). Mirrors shipped `link_observation_files_to_memories`.
- [x] Fix 2a src/githook.rs ingest_one: `%ae` vs in-repo `git config
  user.email` → `author_email`+`authored_locally` in commit_made metadata
  (fail-open).
- [x] Fix 2b src/observe.rs: gate `link_commit_to_open_memories` on
  `metadata.authored_locally` (absent→local, backward-compatible); watermark
  unaffected.
- Accepted-not-fixed (anti-bias, no invented work): COMMIT_PROTECT_DAYS=90;
  strength non-idempotence (soft/clamped/by-design); semantic matcher (now
  optional enhancement, not a blocker once Fix 1 lands).
- [x] Build clean (2m26s, no errors) + service restarted on new binary.
- [x] Verified:
  - Fix1: commit on a 21-memory hot file → NO grounding (none immortalized);
    commit on a 1-memory specific file → `strengthened 1` (normal grounding
    intact). Per-file guard works, no under-protection.
  - Fix2a: captured adapter POST bodies — local commit →
    `("me@test", authored_locally=true)`; foreign →
    `("teammate@other", false)`. In-repo discriminator correct.
  - Fix2b: 6-line backward-compatible gate (absent flag → true); compiled,
    mirrors the proven revert-gate idiom.
  - Regression: health stable (status healthy), no errors/panics.

## STATUS: COMPLETE. Both phases shipped & verified. Nothing committed
(no commit requested). plan-commit-grounding.md = scratch.

## Phase 3 — commits viewer fix: DONE & VERIFIED
Root cause: commits.rs diff()/list() read `observation.project` (dead column,
same family as K1) → every commit's diff = "commit not found". Fix: traverse
`session_id.project` (proven observe.rs:597 pattern); 2 query-string edits in
src/commits.rs, no schema/migration. Verified live: real HEAD ingest → diff
renders (35KB); project-filtered list → 3 rows; fabricated sha → honest git
error; health stable. Build clean, restarted.

## Phase 1 (core layer) — DONE & VERIFIED (HEAD post-605f9fe, uncommitted)
- plans::current_id added; observe.rs commit_made: deterministic
  plan--implemented_by(via:declared)-->commit (gated !revert && authored_locally);
  commits_for demoted via:inferred; cfg(code) commit--touches_code-->code_chunk.
- trace.rs: trace_multi + causal(); TraceReq.ids; lib::timeline_causal + multi-seed
  trace; GET /api/v1/agent/timeline/causal; hifz_timeline_causal + hifz_trace ids.
- Builds clean (release default + --features code). restart-service ok.
- Verified live: implemented_by edge count=1 via:declared with active plan;
  none without; causal timeline ordered (plan@t -> commit@t); multi-seed count 2;
  flat timeline + health unaffected. commits_for via:inferred = code-verified
  (synthetic payload title:"c" starves the BM25 matcher; real .mjs title differs).
## Next: Phase 2 — adapter plan-activation keystone.

## Phases 2 & 3 — DONE & VERIFIED
- P2 keystone: plan-capture.mjs now POSTs /memories THEN /agent/plans/activate
  (was inert). session-start.mjs appends graph-assembled "Active decision —
  provenance (time-ordered)" from /timeline/causal of the active plan.
- P2 scope decision (anti-confirmation-bias): post-tool-failure.mjs left
  unchanged — the flat similar-failure search there is correct; no clean causal
  seed at failure time; the plan's "replace with trace" was speculative. Skipped
  deliberately, documented (not silently).
- Verified end-to-end: simulated Write→plan-capture → current_plan returns the
  plan (activate fired); commit_made → plan--implemented_by(via:declared)-->commit
  count=1; session-start emits the provenance block. Revert commit with active
  plan → implemented_by count=0 (correctly demoted). atlas untouched (no atlas
  files changed — deliberately separate). Health healthy, no regression.
- Builds: Rust (P1) clean release default + --features code; daemon restarted.
  P2 = .mjs only (no rebuild). Nothing committed (no commit requested).
- NOTE: Cargo.toml/Makefile/benchmark/code_retrieval_bench.rs/docs were NOT
  modified by this implementation (pre-existing/linter in the work area).
