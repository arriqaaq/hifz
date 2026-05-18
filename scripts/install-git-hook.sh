#!/bin/sh
# Install the hifz git post-commit hook into a repository.
#
#   sh scripts/install-git-hook.sh [TARGET_REPO]
#
# TARGET_REPO defaults to the current git repo. Idempotent. If the repo
# already has a non-hifz post-commit hook, it is preserved and chained
# (run first), so this never clobbers existing automation.

set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CANON="$SCRIPT_DIR/../adapters/claude-code/scripts/post-commit-hook.sh"
[ -f "$CANON" ] || { echo "ERROR: canonical hook not found at $CANON" >&2; exit 1; }
CANON="$(cd "$(dirname "$CANON")" && pwd)/$(basename "$CANON")"
chmod +x "$CANON" 2>/dev/null || true

TARGET="${1:-$(git rev-parse --show-toplevel 2>/dev/null || true)}"
[ -n "$TARGET" ] || { echo "ERROR: not a git repo and no TARGET_REPO given" >&2; exit 1; }
HOOKS_DIR="$TARGET/.git/hooks"
[ -d "$HOOKS_DIR" ] || { echo "ERROR: $HOOKS_DIR missing (is $TARGET a git repo?)" >&2; exit 1; }
DEST="$HOOKS_DIR/post-commit"
MARKER="# hifz-post-commit-dispatcher"

if [ -f "$DEST" ] && grep -q "$MARKER" "$DEST" 2>/dev/null; then
  echo "==> hifz post-commit hook already installed at $DEST (refreshed)"
elif [ -f "$DEST" ]; then
  # Preserve and chain the user's existing hook.
  mv "$DEST" "$HOOKS_DIR/post-commit.local"
  echo "==> existing post-commit hook preserved as post-commit.local (chained)"
fi

cat > "$DEST" <<EOF
#!/bin/sh
$MARKER — installed by hifz scripts/install-git-hook.sh
# Runs any pre-existing hook first, then the hifz commit_made emitter.
[ -x "\$(dirname "\$0")/post-commit.local" ] && "\$(dirname "\$0")/post-commit.local" "\$@"
exec "$CANON" "\$@"
EOF
chmod +x "$DEST"

echo "==> installed: $DEST -> $CANON"
echo "==> every commit in $TARGET now emits a commit_made observation"
echo "    (server: \${HIFZ_URL:-http://localhost:3111}; best-effort, never blocks a commit)"
