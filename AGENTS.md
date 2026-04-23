# hifz — Agent Instructions

## Architecture

hifz is a persistent memory system for AI coding agents, built in Rust.

- **Runtime**: Two processes — `hifz serve` (REST API on port 3111) and `hifz mcp` (stdio JSON-RPC proxy, spawned by Claude Code)
- **Storage**: Embedded SurrealDB (`kv-surrealkv` for persistent, `kv-mem` for testing)
- **Embeddings**: fastembed (`all-MiniLM-L6-v2`, 384-dim, local, no API key)
- **Search**: Hybrid BM25 full-text + HNSW vector + RRF fusion
- **LLM** (optional): Ollama for compression and consolidation
- **Plugin**: Hooks (auto-capture) + skills (`/recall`, `/remember`, `/forget`, `/session-history`)
- **Build**: `cargo build` (edition 2024)
- **Test**: `cargo test`

## Three-Part Integration

hifz has three independent parts that all talk to the same REST server:

### 1. REST Server (`hifz serve`)
The core process. Holds embedded SurrealDB, fastembed, and all HTTP endpoints. Must be running for anything else to work.

```bash
hifz serve --db-path ~/.hifz/data   # persistent
hifz serve --memory                  # ephemeral (testing)
```

### 2. MCP Server (`hifz mcp`)
A thin stdio-to-HTTP proxy. Claude Code spawns it via `.mcp.json` and talks to it over stdio (JSON-RPC). It converts MCP tool calls into HTTP requests to the REST server. Has no database or logic of its own.

**Purpose**: Gives the agent 8 on-demand tools (`hifz_save`, `hifz_recall`, `hifz_search`, etc.). The agent must explicitly call these.

### 3. Plugin (hooks + skills)
The `.mjs` scripts in `adapters/claude-code/scripts/` are shell hooks that Claude Code executes at lifecycle events. They are Node.js (not Rust) because Claude Code hooks must be executable shell commands.

Each hook reads JSON from stdin, POSTs to the REST server, and exits:
- `PostToolUse` fires → `post-tool-use.mjs` POSTs `{tool_name, tool_input, tool_output}` to `/hifz/observe`
- `SessionStart` fires → `session-start.mjs` POSTs to `/hifz/session/start` → if `HIFZ_INJECT_CONTEXT=true`, writes context back to stdout for Claude Code to consume

**Purpose**: Passive auto-capture (writes) and automatic context injection (reads on session start). No agent action needed.

### Why both MCP and hooks exist

| | MCP tools | Plugin hooks |
|---|---|---|
| **Triggered by** | Agent explicitly calling a tool | Claude Code lifecycle events (automatic) |
| **Direction** | Agent asks for data on demand | Data pushed to server without agent action |
| **Example** | "search for authentication" | "you just read config.rs" → auto-captured |
| **Protocol** | stdio JSON-RPC | Shell command (stdin JSON → HTTP POST) |

Without hooks, nothing gets captured automatically. Without MCP, the agent can't search or save on demand. They complement each other.

### What is automatic vs manual

| Feature | Automatic? | How |
|---|---|---|
| Capture tool usage | Yes | Plugin hooks (PostToolUse, etc.) |
| Capture prompts | Yes | Plugin hook (UserPromptSubmit) |
| Context injection on session start | Yes | Plugin hook (SessionStart) + `HIFZ_INJECT_CONTEXT=true` |
| Save important insights | Best-effort | Agent follows CLAUDE.md instructions to call `hifz_save` |
| Search mid-session | Best-effort | Agent follows CLAUDE.md instructions to call `hifz_recall` |

## Source Layout

| Module | Purpose |
|---|---|
| `src/main.rs` | CLI parser (clap), server startup |
| `src/lib.rs` | Module exports |
| `src/web/mod.rs` | Axum router, AppState, `serve()` |
| `src/web/api.rs` | REST endpoint handlers |
| `src/mcp/mod.rs` | Stdio JSON-RPC MCP server (proxy to REST) |
| `src/mcp/tools.rs` | 8 MCP tool handlers + definitions |
| `src/db.rs` | SurrealDB connection + schema |
| `src/models.rs` | Data types (Session, Observation, Memory, etc.) |
| `src/observe.rs` | Observation capture pipeline (dedup -> compress -> embed -> store) |
| `src/search.rs` | BM25 + vector + RRF hybrid search |
| `src/embed.rs` | FastEmbed wrapper |
| `src/compress.rs` | Synthetic + LLM compression |
| `src/context.rs` | Session-start context generation |
| `src/dedup.rs` | SHA-256 dedup with 5-min TTL |
| `src/remember.rs` | Save/delete long-term memories |
| `src/forget.rs` | GC: TTL expiry, contradiction detection, low-value eviction |
| `src/digest.rs` | Project intelligence (top concepts, files, stats) |
| `src/consolidate.rs` | 4-tier memory consolidation pipeline |
| `src/config.rs` | Env var loading from `~/.hifz/.env` |
| `src/ollama.rs` | Ollama HTTP client |
| `src/prompts.rs` | LLM prompt templates |

## User Commands

Slash commands (type these in Claude Code):

| Command | What it does |
|---|---|
| `/remember [what]` | Save an insight or decision to long-term memory |
| `/recall [query]` | Search hifz for past observations and learnings |
| `/forget [what]` | Delete specific observations or memories |
| `/session-history` | Show what happened in recent past sessions |

MCP tools (called by the agent, or ask the agent to use them):

| Tool | What it does |
|---|---|
| `hifz_save` | Save insight with title, content, type, concepts, files |
| `hifz_recall` | Search observations and memories by query |
| `hifz_search` | Hybrid BM25 + vector search (better for semantic queries) |
| `hifz_delete` | Delete a memory by ID |
| `hifz_digest` | Project intelligence — top concepts, files, stats |
| `hifz_sessions` | List recent sessions |
| `hifz_timeline` | Chronological observations (optionally filter by session) |
| `hifz_export` | Export all memory data |

## Plugin Layout

| Path | Purpose |
|---|---|
| `adapters/claude-code/.claude-plugin/plugin.json` | Plugin manifest |
| `adapters/claude-code/.claude-plugin/marketplace.json` | Marketplace manifest (for `claude plugin install`) |
| `adapters/claude-code/.mcp.json` | MCP server config (auto-loaded by Claude Code) |
| `adapters/claude-code/hooks/hooks.json` | 14 hook entries (SessionStart, PostToolUse, PostCompact, plan-capture, etc.) |
| `adapters/claude-code/scripts/*.mjs` | Hook scripts — read JSON from stdin, POST to REST server |
| `adapters/claude-code/skills/recall/SKILL.md` | `/recall` slash command |
| `adapters/claude-code/skills/remember/SKILL.md` | `/remember` slash command |
| `adapters/claude-code/skills/forget/SKILL.md` | `/forget` slash command |
| `adapters/claude-code/skills/session-history/SKILL.md` | `/session-history` slash command |

## Setup for Other Projects

To use hifz in a project:

1. Start the REST server (`hifz serve --db-path ~/.hifz/data`)
2. Add `.mcp.json` to the project root pointing to the hifz binary
3. Install the plugin: `/plugin marketplace add /path/to/hifz/plugin` then `/plugin install hifz@hifz`
4. Set `HIFZ_INJECT_CONTEXT=true` in `.claude/settings.local.json` env for auto-context injection
5. Restart Claude Code

## Consistency Rules

**When adding REST endpoints:**
1. `src/web/api.rs` — handler function
2. `src/web/mod.rs` — `.route(...)` registration

**When adding MCP tools:**
1. `src/mcp/tools.rs` — match arm in `call_tool()` + entry in `tool_defs()`

**When adding DB tables:**
1. `src/db.rs` — DEFINE TABLE/FIELD statements in SCHEMA
2. `src/models.rs` — corresponding Rust struct

**When adding hooks:**
1. `adapters/claude-code/hooks/hooks.json` — hook definition
2. `adapters/claude-code/scripts/<name>.mjs` — hook script

**When adding skills:**
1. `adapters/claude-code/skills/<name>/SKILL.md` — skill definition
2. `adapters/claude-code/.claude-plugin/plugin.json` — ensure `skills` path includes it

**When bumping version:**
1. `Cargo.toml` — version field
2. `adapters/claude-code/.claude-plugin/plugin.json` — version field

## Code Patterns

### REST endpoint handler
```rust
pub async fn my_endpoint(
    State(state): State<AppState>,
    Json(body): Json<MyReq>,
) -> Json<serde_json::Value> {
    // use state.db, state.embedder, etc.
    Json(serde_json::json!({"status": "ok"}))
}
```

### MCP tool handler
```rust
"my_tool" => {
    let arg = args.get("key").and_then(|v| v.as_str()).unwrap_or("");
    // do work
    serde_json::json!({"result": "value"})
}
```

### Hook scripts
Hook scripts in `adapters/claude-code/scripts/` are standalone Node.js `.mjs` files. They read JSON from stdin, POST to the REST API, and exit. Always use `try/catch` with `AbortSignal.timeout()`.

## Config

Config is loaded from `~/.hifz/.env` (file) and process environment (fallback).

| Env var | Default | Purpose |
|---|---|---|
| `HIFZ_PORT` | 3111 | REST API port |
| `HIFZ_AUTO_COMPRESS` | false | Use Ollama for compression |
| `OLLAMA_URL` | (none) | Ollama endpoint |
| `OLLAMA_MODEL` | qwen2.5:7b | LLM model |
| `CONSOLIDATION_ENABLED` | true | 4-tier consolidation |
| `TOKEN_BUDGET` | 2000 | Context injection token limit |
| `MAX_OBS_PER_SESSION` | 500 | Max observations per session |
| `HIFZ_INJECT_CONTEXT` | false | Auto-inject context on session start (set in Claude Code settings env) |

## Current Stats (v0.9.0)

- 8 MCP tools
- 16 REST endpoints
- 14 hooks, 4 skills
