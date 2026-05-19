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
