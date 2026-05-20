.PHONY: build frontend backend server dev stop check test smoke status install uninstall sync-ontology check-ontology install-service uninstall-service restart-service service-status install-git-hook backfill-commits code-retrieval-bench

HIFZ_BIN  := ./target/debug/hifz
DB_PATH   := ~/.hifz/data
PORT      := 3111
LABEL     := com.hifz.server

# --- Build ---

build: backend frontend

backend:
	cargo build

frontend:
	cd website && npm install
	cd website && rm -rf .svelte-kit _build_stage
	cd website && SVELTE_OUT=_build_stage npm run build
	cd website && rm -rf build.old && (mv build build.old 2>/dev/null || true) && mv _build_stage build && rm -rf build.old

check:
	cargo check
	cargo test --lib

# Phase 9.3: regenerate the website + Pi extension TS ontology mirrors
# from src/models.rs. CI runs `make check-ontology` which fails on drift.
sync-ontology:
	@node scripts/sync-ontology.mjs

check-ontology:
	@node scripts/sync-ontology.mjs --check

test:
	cargo test

# --- Run ---

server: build
	$(HIFZ_BIN) serve --db-path $(DB_PATH)

dev: build
	@echo "Starting hifz on http://localhost:$(PORT) ..."
	@$(HIFZ_BIN) serve --db-path $(DB_PATH) &
	@sleep 3
	@echo "Server running. UI: http://localhost:$(PORT)  API: http://localhost:$(PORT)/hifz/*"
	@echo "Use 'make stop' to shut down."

stop:
	@pkill -f "hifz serve" 2>/dev/null && echo "Server stopped" || echo "No server running"

smoke:
	@./scripts/smoke-test.sh

status:
	@curl -s http://localhost:$(PORT)/hifz/health | python3 -m json.tool 2>/dev/null || echo "Server not running"

# --- Install (registers the repo plugin as a local directory marketplace) ---
# Claude Code does NOT auto-discover symlinked dirs under ~/.claude/plugins.
# The supported, scriptable mechanism is a directory-source marketplace plus
# enabledPlugins in ~/.claude/settings.json (the exact config that makes hifz
# work today). Idempotent + atomic: settings.json is never clobbered on error.
PLUGIN_DIR      := $(CURDIR)/adapters/claude-code
CLAUDE_SETTINGS := $(HOME)/.claude/settings.json

install:
	@command -v jq >/dev/null || { echo "ERROR: jq required (brew install jq)"; exit 1; }
	@[ -f "$(PLUGIN_DIR)/.claude-plugin/plugin.json" ] || { echo "ERROR: $(PLUGIN_DIR)/.claude-plugin/plugin.json not found"; exit 1; }
	@mkdir -p "$(dir $(CLAUDE_SETTINGS))"
	@[ -f "$(CLAUDE_SETTINGS)" ] || echo '{}' > "$(CLAUDE_SETTINGS)"
	@T=$$(mktemp); jq --arg p "$(PLUGIN_DIR)" \
	  '.extraKnownMarketplaces.hifz.source = {source:"directory", path:$$p} | .enabledPlugins."hifz@hifz" = true' \
	  "$(CLAUDE_SETTINGS)" > "$$T" && mv "$$T" "$(CLAUDE_SETTINGS)" \
	  || { rm -f "$$T"; echo "ERROR: jq failed; $(CLAUDE_SETTINGS) left unchanged"; exit 1; }
	@echo "==> hifz registered: marketplace=directory:$(PLUGIN_DIR), enabledPlugins[hifz@hifz]=true"
	@echo "==> In Claude Code run /reload-plugins (or start a fresh session) to load hooks + /hifz:* skills."
	@echo "==> For the always-on REST server used by hooks/MCP: make install-service"
	@echo "==> For commit-grounding on human/terminal commits: make install-git-hook"
	@echo ""
	@echo "==> This repo's ./.mcp.json already wires the hifz MCP server. For OTHER projects, add to their .mcp.json:"
	@echo '    { "mcpServers": { "hifz": { "command": "$(CURDIR)/target/release/hifz", "args": ["mcp"] } } }'

uninstall:
	@command -v jq >/dev/null || { echo "ERROR: jq required"; exit 1; }
	@[ -f "$(CLAUDE_SETTINGS)" ] || { echo "==> nothing to do ($(CLAUDE_SETTINGS) absent)"; exit 0; }
	@T=$$(mktemp); jq 'del(.enabledPlugins."hifz@hifz") | del(.extraKnownMarketplaces.hifz)' \
	  "$(CLAUDE_SETTINGS)" > "$$T" && mv "$$T" "$(CLAUDE_SETTINGS)" \
	  || { rm -f "$$T"; echo "ERROR: jq failed; $(CLAUDE_SETTINGS) left unchanged"; exit 1; }
	@echo "==> hifz unregistered from $(CLAUDE_SETTINGS). Run /reload-plugins (or restart) to fully unload."

# --- Git commit-grounding hook ---
# Installs a git post-commit hook so EVERY commit (human or Claude) emits a
# `commit_made` observation. Without this, commit-grounding only sees commits
# Claude itself runs via its Bash tool. Idempotent; chains any existing hook.
install-git-hook:
	@sh scripts/install-git-hook.sh

# Backfill commit-grounding from this repo's real git history (oldest→newest).
# Mutates the live store (memory strengths); idempotent (observe dedups).
backfill-commits:
	@sh scripts/backfill-commits.sh .

# --- Always-on service (macOS launchd LaunchAgent) ---
# Runs target/release/hifz serve as a background daemon: starts at
# login/boot, restarts on crash, survives reboot/logout. This is the
# persistent path; `make dev` is foreground/testing only and conflicts
# with the service on port $(PORT).

# `frontend` first: the daemon serves website/build as the SPA, so a stale
# or missing build means /maktab (and every deep route) 404s / serves old UI.
install-service: frontend
	@echo "==> Building release binary (with maktab)..."
	cargo build --release --features maktab
	@echo "==> Installing launchd LaunchAgent ($(LABEL))..."
	@HIFZ_PORT=$(PORT) sh scripts/install-service.sh

uninstall-service:
	@sh scripts/uninstall-service.sh

# Roll a rebuilt binary into the running daemon. ProgramArguments[0] is a
# fixed path, so rebuilding in place + kickstart relaunches from the new
# binary. kickstart hard-fails if the service is not loaded.
# `frontend` first for the same reason as install-service (the daemon serves
# website/build; a stale build serves the old UI after restart).
restart-service: frontend
	cargo build --release --features maktab
	@launchctl kickstart -k "gui/$$(id -u)/$(LABEL)" && echo "hifz service restarted"

service-status:
	@launchctl print "gui/$$(id -u)/$(LABEL)" 2>/dev/null | grep -E "state|pid|last exit" || echo "service not loaded — run 'make install-service'"
	@curl -fsS --max-time 2 http://127.0.0.1:$(PORT)/api/v1/livez 2>/dev/null && echo " <- /livez OK" || echo "/livez FAILED"

# Code-retrieval correctness diagnosis: does hifz code_search find the right
# function, and when it doesn't, why (recoverable vs deep). Diagnose-only.
# See docs/eval/code-retrieval.md.
code-retrieval-bench:
	cargo run --release --bin code-retrieval-bench -- --root .

