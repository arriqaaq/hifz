#!/bin/sh
# Backfill commit-grounding from a repo's real git history.
#
#   sh scripts/backfill-commits.sh [REPO] [--limit N]
#
# Replays every commit (or the most recent N) oldest→newest as a
# `commit_made` observation to $HIFZ_URL, so commit-grounding works on real
# data immediately instead of only on commits made after the hook landed.
# Reuses the exact payload builder the live post-commit hook uses.
#
# SIDE EFFECT: this mutates the live hifz store — ground::on_commit_signal
# changes memory `strength`. Idempotent: observe dedups by content hash, so
# re-running does not double-count. Bounded curl timeout; never hangs.

set -eu

REPO="."
LIMIT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --limit) LIMIT="${2:-}"; shift 2 ;;
    -h|--help)
      echo "usage: backfill-commits.sh [REPO] [--limit N]"; exit 0 ;;
    *) REPO="$1"; shift ;;
  esac
done

HIFZ_URL="${HIFZ_URL:-http://localhost:3111}"
command -v jq   >/dev/null 2>&1 || { echo "ERROR: jq required" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "ERROR: curl required" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIB="$SCRIPT_DIR/../adapters/claude-code/scripts/lib-commit-payload.sh"
[ -f "$LIB" ] || { echo "ERROR: $LIB not found" >&2; exit 1; }

REPO="$(cd "$REPO" && git rev-parse --show-toplevel)" \
  || { echo "ERROR: $REPO is not a git repo" >&2; exit 1; }
cd "$REPO"
. "$LIB"

if [ -n "$LIMIT" ]; then
  # most recent N, replayed oldest→newest
  SHAS="$(git rev-list -n "$LIMIT" HEAD | awk '{a[NR]=$0} END{for(i=NR;i>=1;i--)print a[i]}')"
else
  SHAS="$(git rev-list --reverse HEAD)"
fi
N="$(printf '%s\n' "$SHAS" | grep -c . || true)"

echo "==> Backfilling $N commit(s) from $REPO into $HIFZ_URL"
echo "    SIDE EFFECT: this changes memory strengths in the live store (idempotent)."

ok=0
for sha in $SHAS; do
  payload="$(_hifz_build_payload "$sha" git-backfill git-backfill 2>/dev/null)" || continue
  [ -n "$payload" ] || continue
  if curl -s -m 5 -X POST "$HIFZ_URL/api/v1/agent/observe" \
       -H "Content-Type: application/json" -d "$payload" >/dev/null 2>&1; then
    ok=$((ok + 1))
  fi
done

echo "==> Posted $ok/$N commit_made observations."
echo "    Re-run: cargo run --release --bin corpus-memory-bench -- --project <name>"
