.PHONY: build frontend backend server dev stop check test smoke status install

HIFZ_BIN  := ./target/debug/hifz
DB_PATH   := ~/.hifz/data
PORT      := 3111

# --- Build ---

build: backend frontend

backend:
	cargo build

frontend:
	cd website && npm install && rm -rf .svelte-kit build && npm run build

check:
	cargo check
	cargo test --lib

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

