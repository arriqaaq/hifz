#!/bin/sh
# hifz git post-commit hook.
#
# WHY THIS EXISTS: the Claude-Code adapter (post-tool-use.mjs) only emits a
# `commit_made` observation when Claude *itself* runs `git commit` through its
# Bash tool. Commits made by a human in their own terminal (or any non-Claude
# process) never pass through that hook, so commit-grounding — hifz's core
# "rank by what shipped" signal — stays inert for normal workflows. This
# git-layer hook captures EVERY commit regardless of who made it and POSTs the
# same `commit_made` payload (built by lib-commit-payload.sh, shared with the
# git-history backfill so they are byte-identical).
#
# Contract: must never block or fail a commit. Best-effort, backgrounded,
# bounded by a short curl timeout (the project's "no high timeouts" rule).
# Requires `jq`, `curl`, `git`; exits 0 silently if any is missing/down.
#
# Installed by scripts/install-git-hook.sh (chains any pre-existing hook).

HIFZ_URL="${HIFZ_URL:-http://localhost:3111}"

command -v jq   >/dev/null 2>&1 || exit 0
command -v curl >/dev/null 2>&1 || exit 0

. "$(dirname "$0")/lib-commit-payload.sh" 2>/dev/null || exit 0

sha="$(git rev-parse HEAD 2>/dev/null)" || exit 0
payload="$(_hifz_build_payload "$sha" git-post-commit git-post-commit-hook)" || exit 0
[ -n "$payload" ] || exit 0

# Fire-and-forget: never delay or fail the commit.
( curl -s -m 3 -X POST "$HIFZ_URL/api/v1/agent/observe" \
    -H "Content-Type: application/json" -d "$payload" >/dev/null 2>&1 & ) >/dev/null 2>&1

exit 0
