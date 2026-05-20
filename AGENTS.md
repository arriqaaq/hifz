# hifz — Agent Instructions

## Architecture

hifz is a persistent memory system for AI coding agents, built in Rust.

- **Runtime**: Two processes — `hifz serve` (REST API on port 3111) and `hifz mcp` (stdio JSON-RPC proxy, spawned by Claude Code)
- **Workspace**: a Cargo workspace — the `hifz` binary plus three crates:
  - `crates/kernel` — shared primitives (SurrealDB schema, fastembed, Ollama client, models, code-intelligence core). The dependency sink; no cycles.
  - `crates/memdiff` — zero-dependency presentation layer (renders memory deltas to JSON/text; same struct everywhere).
  - `crates/maktab` — optional corpus-graph product, mounted under `/api/v1/maktab` behind the `maktab` feature.
- **Storage**: Embedded SurrealDB (`kv-surrealkv` for persistent, `kv-mem` for testing); namespace `hifz`, database `main`
- **Embeddings**: fastembed (`all-MiniLM-L6-v2`, 384-dim, local, no API key)
- **Search**: Hybrid BM25 full-text + HNSW vector + RRF fusion (default `k=10`)
- **LLM** (optional): Ollama for compression, evolution, consolidation, and reranking. Everything works deterministically without it.
- **Plugin**: Hooks (auto-capture) + skills (`/recall`, `/remember`, `/forget`, `/session-history`)
- **Build**: `cargo build` (edition 2024); `cargo build --release --features maktab` for the full daemon
- **Test**: `cargo test`

## Three-Part Integration

hifz has three independent parts that all talk to the same REST server:

### 1. REST Server (`hifz serve`)
The core process. Holds embedded SurrealDB, fastembed, and all HTTP endpoints. Must be running for anything else to work. Routes are split into three groups:

- `/api/v1/*` — core memory & code (`/memories`, `/search`, `/trace`, `/consolidate`, `/forget-gc`, `/core/{project}`, `/code/*`, `/export`, …)
- `/api/v1/agent/*` — capture & trajectory pipeline (`/observe`, `/sessions`, `/observations`, `/timeline`, `/runs`, `/commits`, `/plans`, `/digest`, `/usage`)
- `/api/v1/maktab/*` — corpus-graph product (only mounted when built `--features maktab`)

```bash
hifz serve --db-path ~/.hifz/data   # persistent
hifz serve --memory                  # ephemeral (testing)
```

### 2. MCP Server (`hifz mcp`)
A thin stdio-to-HTTP proxy. Claude Code spawns it via `.mcp.json` and talks to it over stdio (JSON-RPC). It converts MCP tool calls into HTTP requests to the REST server. Has no database or logic of its own.

**Purpose**: gives the agent on-demand tools. It advertises a curated default surface of **13 core tools** (`hifz_save`, `hifz_recall`, `hifz_search`, `hifz_delete`, `hifz_sessions`, the four `*_plan` tools, and the four `hifz_code_*`/`hifz_link_*` tools) and can dispatch the **full ~31-tool hifz surface** (including `hifz_digest`, `hifz_timeline`, `hifz_timeline_causal`, `hifz_runs`, `hifz_commits`, `hifz_core_get/edit`, `hifz_evolve`, `hifz_trace`, `hifz_view`, `hifz_export`, …). When built `--features maktab`, six `maktab_*` tools are also advertised. The agent must explicitly call these.

### 3. Plugin (hooks + skills)
The scripts in `adapters/claude-code/scripts/` are shell hooks Claude Code executes at lifecycle events. They are authored in TypeScript and shipped as compiled Node.js `.mjs` (Claude Code hooks must be executable shell commands).

Each hook reads JSON from stdin, POSTs to the REST server, and exits:
- `PostToolUse` fires → `post-tool-use.mjs` POSTs `{tool_name, tool_input, tool_output}` to `/api/v1/agent/observe`
- `SessionStart` fires → `session-start.mjs` GETs the project warmup digest from `/api/v1/agent/sessions/{id}/warmup` and writes it back to stdout for Claude Code to consume

**Purpose**: passive auto-capture (writes) and automatic context injection (reads on session start). No agent action needed.

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
| Context injection on session start | Yes | Plugin hook (SessionStart) |
| Save important insights | Best-effort | Agent follows CLAUDE.md instructions to call `hifz_save` |
| Search mid-session | Best-effort | Agent follows CLAUDE.md instructions to call `hifz_recall` |

## Source Layout

Shared primitives live in `crates/kernel`; the daemon-specific pipeline lives in `src/`.

| Module | Purpose |
|---|---|
| `src/main.rs` | CLI parser (clap): `serve`, `mcp`, `save`, `code-index`, `code-graph`, `code-gc`, `export`, `import`, `reindex`, `git-hook` |
| `src/lib.rs` | Library facade (`Hifz` struct), module exports |
| `src/web/mod.rs` | Axum router (`/api/v1`, `/api/v1/agent`, `/api/v1/maktab`), `AppState`, `serve()`, static SvelteKit serving |
| `src/web/api.rs` | REST endpoint handlers (~40, pure JSON marshalling around `Hifz` methods) |
| `src/mcp/mod.rs` | Stdio JSON-RPC MCP server (proxy to REST) |
| `src/mcp/tools.rs` | Tool dispatch + `CORE_TOOLS` allowlist + `tool_defs()` |
| `src/observe.rs` | Observation capture pipeline (dedup → compress → embed → store) |
| `src/enrich.rs` | Memory write pipeline: extraction → optional LLM enrich → embed → persist → link |
| `src/remember.rs` | Deterministic memory save (thin wrapper over `enrich`) |
| `src/link.rs` | Write-time edge generation (embedding KNN, keyword/file Jaccard, entity links) |
| `src/evolve.rs` | LLM neighbour refinement + supersession (gated by `HIFZ_LLM_EVOLVE`) |
| `src/ground.rs` | Commit-grounding: strengthen memories on committed files, weaken on revert |
| `src/search.rs` | BM25 + vector + RRF hybrid search, optional rerank, diversification |
| `src/rank.rs` | Rust-side scoring (`strength × exp(-age/30) × (1 + 0.1·min(access,20))`) |
| `src/rerank.rs`, `src/llm_rerank.rs` | ONNX cross-encoder / LLM listwise reranking (both optional) |
| `src/consolidate.rs` | 4-tier consolidation (writes `semantic_fact`/`procedure` memories + decay) |
| `src/context.rs` | Session-start context generation |
| `src/dedup.rs` | SHA-256 dedup with 5-min TTL |
| `src/forget.rs` | GC: TTL expiry, contradiction detection, low-value eviction |
| `src/plans.rs`, `src/session.rs`, `src/run.rs`, `src/timeline.rs`, `src/trace.rs`, `src/commits.rs` | Trajectory & graph surfaces |
| `src/code/` | Code indexing, search, memory↔code linking, GC, file watcher |
| `crates/kernel/src/db.rs` | SurrealDB connection + `DEFINE TABLE/FIELD` schema |
| `crates/kernel/src/models.rs` | Data types — `Memory`, `Observation`, `Session`, `Run`, `Category`, `EdgeRelation`, … |
| `crates/kernel/src/embed.rs` | fastembed wrapper |
| `crates/kernel/src/ollama.rs` | Ollama HTTP client |
| `crates/kernel/src/config.rs` | Env-var loading from `~/.hifz/.env` |
| `crates/kernel/src/code_parse/` | Code-intelligence core (tree-sitter walkers, scope-qualified identities, resolver) |

## User Commands

Slash commands (type these in Claude Code):

| Command | What it does |
|---|---|
| `/remember [what]` | Save an insight or decision to long-term memory |
| `/recall [query]` | Search hifz for past observations and learnings |
| `/forget [what]` | Delete specific observations or memories |
| `/session-history` | Show what happened in recent past sessions |

Commonly used MCP tools (called by the agent, or ask the agent to use them):

| Tool | What it does |
|---|---|
| `hifz_save` | Save insight with title, content, category, keywords, files |
| `hifz_recall` | Search observations + memories with graph expansion |
| `hifz_search` | Hybrid BM25 + vector search |
| `hifz_delete` | Delete a memory by ID |
| `hifz_code_index` / `hifz_code_search` | Index / search a repository's code |
| `hifz_link_code` / `hifz_link_symbol` | Link a memory to a file+line range / named symbol |
| `hifz_current_plan` / `hifz_plans` / `hifz_activate_plan` / `hifz_complete_plan` | Plan lifecycle |
| `hifz_digest` / `hifz_sessions` / `hifz_timeline` | Project intelligence and trajectory |

## Plugin Layout

| Path | Purpose |
|---|---|
| `adapters/claude-code/.claude-plugin/plugin.json` | Plugin manifest |
| `adapters/claude-code/.claude-plugin/marketplace.json` | Marketplace manifest (for `claude plugin install`) |
| `adapters/claude-code/.mcp.json` | MCP server config (auto-loaded by Claude Code) |
| `adapters/claude-code/hooks/hooks.json` | Hook registrations — 10 lifecycle events (SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, SubagentStop, PreCompact, Notification, TaskCompleted, SessionEnd), 28 command entries |
| `adapters/claude-code/scripts/*.mjs` | Hook scripts (TypeScript compiled to Node.js) — read JSON from stdin, POST to REST server |
| `adapters/claude-code/skills/recall/SKILL.md` | `/recall` slash command |
| `adapters/claude-code/skills/remember/SKILL.md` | `/remember` slash command |
| `adapters/claude-code/skills/forget/SKILL.md` | `/forget` slash command |
| `adapters/claude-code/skills/session-history/SKILL.md` | `/session-history` slash command |

## Setup for Other Projects

To use hifz in a project:

1. Start the REST server (`hifz serve --db-path ~/.hifz/data`)
2. Add `.mcp.json` to the project root pointing to the hifz binary
3. Install the plugin: `/plugin marketplace add /path/to/hifz` then `/plugin install hifz@hifz`
4. Restart Claude Code

For non–Claude Code clients: anything that can `POST /api/v1/memories` and `POST /api/v1/search` is a first-class memory client; add `POST /api/v1/agent/observe` to populate the capture layer.

## Consistency Rules

**When adding REST endpoints:**
1. `src/web/api.rs` — handler function
2. `src/web/mod.rs` — `.route(...)` registration (under the right `/api/v1` or `/api/v1/agent` nest)

**When adding MCP tools:**
1. `src/mcp/tools.rs` — match arm in `call_tool()` + entry in `tool_defs()` (+ `CORE_TOOLS` if it should be advertised by default)

**When adding DB tables:**
1. `crates/kernel/src/db.rs` — `DEFINE TABLE`/`DEFINE FIELD` statements
2. `crates/kernel/src/models.rs` — corresponding Rust struct/enum

**When changing the `Category` or `EdgeRelation` enums:**
1. `crates/kernel/src/models.rs` — the enum
2. Run `node scripts/sync-ontology.mjs` to regenerate the TS mirrors (`website/src/lib/ontology.ts`, `adapters/pi-extension/src/ontology.ts`)
3. Update `docs/ontology.md`

**When adding hooks:**
1. `adapters/claude-code/hooks/hooks.json` — hook definition
2. `adapters/claude-code/scripts/<name>` — hook script (TypeScript source compiled to `.mjs`)

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
Hook scripts in `adapters/claude-code/scripts/` are standalone Node.js `.mjs` files (compiled from TypeScript). They read JSON from stdin, POST to the REST API, and exit. Always use `try/catch` with `AbortSignal.timeout()`.

## Config

Config is loaded from `~/.hifz/.env` (file) and process environment (fallback).

| Env var | Default | Purpose |
|---|---|---|
| `HIFZ_PORT` | 3111 | REST API port |
| `HIFZ_URL` | http://localhost:3111 | REST target for the `hifz mcp` proxy |
| `HIFZ_AUTO_COMPRESS` | false | Use Ollama for observation compression |
| `HIFZ_LLM_EVOLVE` | false | Enable LLM enrichment + evolution at memory insert |
| `HIFZ_CODE_WATCH` | false | Enable the live code re-indexing watcher |
| `HIFZ_CODE_WATCH_ROOTS` | (none) | Watcher roots, `project=/path,...` |
| `OLLAMA_URL` | (none) | Ollama endpoint |
| `OLLAMA_MODEL` | qwen2.5:7b | LLM model |
| `CONSOLIDATION_ENABLED` | true | 4-tier consolidation |
| `TOKEN_BUDGET` | 2000 | Context injection token limit |
| `MAX_OBS_PER_SESSION` | 500 | Max observations per session |

## Current surface

- MCP: 13 advertised core tools + the full ~31-tool hifz surface dispatchable; 6 `maktab_*` tools when built `--features maktab`
- REST: routes under `/api/v1` (core), `/api/v1/agent` (capture/trajectory), and `/api/v1/maktab` (feature-gated)
- Plugin: 10 hook lifecycle events (28 command entries), 4 skills
