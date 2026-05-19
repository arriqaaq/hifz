#!/bin/sh
# Install the hifz REST server as an always-on macOS launchd LaunchAgent.
#
# Deterministic, single path, hard pass/fail:
#   - requires target/release/hifz (no debug fallback)
#   - modern launchctl only (no legacy `load -w`)
#   - exits 0 only if /api/v1/livez answers within the bound; else exits 1
#
# Does NOT build — `make install-service` runs `cargo build --release` first.
# Run standalone only against an already-built release binary.
set -eu

LABEL="com.hifz.server"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${HIFZ_PORT:-3111}"
BIN="$REPO/target/release/hifz"
DB_PATH="$HOME/.hifz/data"
LOG_DIR="$HOME/.hifz/logs"
PLIST_SRC="$REPO/deploy/launchd/$LABEL.plist.template"
PLIST_DST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"
# LLM backend for the daemon (atlas "Ask" RAG + compression/consolidation).
# Env-overridable; defaults to a local Ollama. The model must be pulled
# (`ollama pull <model>`) or completions fail and atlas degrades to
# sources-only. `qwen3:8b` is the locally-available default.
OLLAMA_URL="${OLLAMA_URL:-http://127.0.0.1:11434}"
OLLAMA_MODEL="${OLLAMA_MODEL:-qwen3:8b}"

# 1. Release binary is required — no fallback.
if [ ! -x "$BIN" ]; then
    echo "ERROR: $BIN missing — run 'make install-service' (builds release first)" >&2
    exit 1
fi

# 2. Directories.
mkdir -p "$DB_PATH" "$LOG_DIR" "$HOME/Library/LaunchAgents"

# 3. Render the template (| delimiter — every value contains /).
sed -e "s|__HIFZ_BIN__|$BIN|g" \
    -e "s|__DB_PATH__|$DB_PATH|g" \
    -e "s|__PORT__|$PORT|g" \
    -e "s|__WORKDIR__|$REPO|g" \
    -e "s|__LOG_DIR__|$LOG_DIR|g" \
    -e "s|__OLLAMA_URL__|$OLLAMA_URL|g" \
    -e "s|__OLLAMA_MODEL__|$OLLAMA_MODEL|g" \
    "$PLIST_SRC" > "$PLIST_DST"

# 4. (Re)bootstrap. The pre-clean bootout is the ONLY allow-fail line: on a
#    first install the service is legitimately not loaded yet (idempotency,
#    not error-masking). Everything after this is hard-fail.
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
# `bootout` is asynchronous: launchd tears the job down in the background.
# Running `bootstrap` before teardown finishes fails with
# "Bootstrap failed: 5: Input/output error" AND leaves nothing loaded.
# Wait (bounded) until the label is actually gone before bootstrapping.
i=0
while launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1 && [ "$i" -lt 20 ]; do
    sleep 0.5
    i=$((i + 1))
done
launchctl bootstrap "$DOMAIN" "$PLIST_DST"
launchctl kickstart -k "$DOMAIN/$LABEL"

# 5. Deterministic verify: poll /livez once/sec, bounded. .fastembed_cache is
#    already populated on this machine -> warm start ~1-2s, so 15s is a real
#    bound, not a hopeful one.
i=0
while [ "$i" -lt 15 ]; do
    if curl -fsS --max-time 2 "http://127.0.0.1:$PORT/api/v1/livez" 2>/dev/null | grep -q ok; then
        echo "hifz server up on :$PORT (launchd: $LABEL)"
        exit 0
    fi
    i=$((i + 1))
    sleep 1
done

echo "ERROR: server did not answer /api/v1/livez in 15s — see $LOG_DIR/server.err.log" >&2
exit 1
