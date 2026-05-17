.PHONY: build frontend backend server dev stop check test smoke status install sync-ontology check-ontology install-service uninstall-service restart-service service-status

HIFZ_BIN  := ./target/debug/hifz
DB_PATH   := ~/.hifz/data
PORT      := 3111
LABEL     := com.hifz.server

# --- Build ---

build: backend frontend

backend:
	cargo build

frontend:
	cd website && npm install && rm -rf .svelte-kit build && npm run build

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

# --- Install (symlinks plugin + MCP into Claude Code) ---

install:
	@echo "==> Installing Claude Code plugin hooks..."
	@mkdir -p ~/.claude/plugins/hifz
	@ln -sfn $(CURDIR)/plugin/hooks/hooks.json ~/.claude/plugins/hifz/hooks.json
	@ln -sfn $(CURDIR)/plugin/scripts ~/.claude/plugins/hifz/scripts
	@echo "==> Plugin installed at ~/.claude/plugins/hifz"
	@echo ""
	@echo "==> Add this to .mcp.json in each project that should use hifz:"
	@echo '    {'
	@echo '      "mcpServers": {'
	@echo '        "hifz": {'
	@echo '          "command": "$(CURDIR)/target/debug/hifz",'
	@echo '          "args": ["mcp"]'
	@echo '        }'
	@echo '      }'
	@echo '    }'

# --- Always-on service (macOS launchd LaunchAgent) ---
# Runs target/release/hifz serve as a background daemon: starts at
# login/boot, restarts on crash, survives reboot/logout. This is the
# persistent path; `make dev` is foreground/testing only and conflicts
# with the service on port $(PORT).

install-service:
	@echo "==> Building release binary..."
	cargo build --release
	@echo "==> Installing launchd LaunchAgent ($(LABEL))..."
	@HIFZ_PORT=$(PORT) sh scripts/install-service.sh

uninstall-service:
	@sh scripts/uninstall-service.sh

# Roll a rebuilt binary into the running daemon. ProgramArguments[0] is a
# fixed path, so rebuilding in place + kickstart relaunches from the new
# binary. kickstart hard-fails if the service is not loaded.
restart-service:
	cargo build --release
	@launchctl kickstart -k "gui/$$(id -u)/$(LABEL)" && echo "hifz service restarted"

service-status:
	@launchctl print "gui/$$(id -u)/$(LABEL)" 2>/dev/null | grep -E "state|pid|last exit" || echo "service not loaded — run 'make install-service'"
	@curl -fsS --max-time 2 http://127.0.0.1:$(PORT)/api/v1/livez 2>/dev/null && echo " <- /livez OK" || echo "/livez FAILED"

