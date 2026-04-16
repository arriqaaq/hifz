# hifz

Persistent memory for Claude Code. Built with Rust and SurrealDB.

Your coding agent forgets everything when the session ends. hifz runs in the background, captures what your agent does, and brings it back when the next session starts.

## What it does

- **Captures** tool usage via Claude Code hooks (12 lifecycle events)
- **Indexes** observations with BM25 full-text search + HNSW vector embeddings
- **Searches** with hybrid retrieval (BM25 + vector + RRF fusion) across both observations and saved memories
- **Injects** relevant context automatically at the start of every session
- **Consolidates** memories over time (4-tier: working, episodic, semantic, procedural)
- **Forgets** stale memories automatically (TTL, contradiction detection, decay)

No API keys needed. No cloud dependencies. Runs 100% locally.

## How it works

hifz has two components:

1. **REST server** — the core process that holds SurrealDB, embeddings, and all API endpoints. You run this in a terminal and keep it open.
2. **MCP proxy** — a thin stdio process that Claude Code spawns automatically. It forwards MCP tool calls to the REST server via HTTP.

Plugin hooks (auto-capture) and MCP tools (save/recall/search) both talk to the same REST server.

```
Claude Code
  ├── MCP tools (hifz_recall, hifz_save, etc.)
  │     └── hifz mcp (stdio proxy → HTTP)
  │           └── forwards to REST server
  │
  └── Plugin hooks (PostToolUse, SessionStart, etc.)
        └── POST http://localhost:3111/hifz/observe
              └── same REST server

REST server (single process)
  └── hifz serve [--memory | --db-path]
      ├── SurrealDB (embedded, single instance)
      ├── fastembed (all-MiniLM-L6-v2, 384d)
      ├── BM25 + HNSW + RRF search
      └── All /hifz/* endpoints
```

## Setup

### Step 1: Build

```bash
git clone https://github.com/arriqaaq/hifz.git
cd hifz
cargo build --release
```

This produces the binary at `./target/release/hifz`.

### Step 2: Start the REST server

Open a terminal and start the server. **Keep this terminal open** — the server must be running for everything else to work.

```bash
# Persistent storage (recommended — data survives restarts)
./target/release/hifz serve --db-path ~/.hifz/data

# Or in-memory (for testing — data lost when server stops)
./target/release/hifz serve --memory
```

The server runs on `http://localhost:3111`. Verify it's up:

```bash
curl http://localhost:3111/hifz/health
```

### Step 3: Add the MCP server to your project

In the project where you want hifz memory, create a `.mcp.json` file at the project root:

```json
{
  "mcpServers": {
    "hifz": {
      "command": "/absolute/path/to/hifz",
      "args": ["mcp"]
    }
  }
}
```

Replace `/absolute/path/to/hifz` with the actual path to your binary:
- Release build: `/Users/you/hifz/target/release/hifz`
- Debug build: `/Users/you/hifz/target/debug/hifz`

The MCP proxy defaults to `http://localhost:3111`. To use a different URL:

```json
{
  "mcpServers": {
    "hifz": {
      "command": "/absolute/path/to/hifz",
      "args": ["mcp", "--url", "http://localhost:3111"]
    }
  }
}
```

### Step 4: Install the plugin (hooks + skills)

The plugin gives you auto-capture hooks and slash command skills (`/recall`, `/remember`, `/forget`, `/session-history`).

**Option A: Load for testing (temporary, this session only)**

```bash
claude --plugin-dir /path/to/hifz/plugin
```

**Option B: Install permanently via marketplace**

From inside a Claude Code session, run:

```
/plugin marketplace add /path/to/hifz/plugin
```

Then:

```
/plugin install hifz@hifz
```

You can verify it installed by checking your `~/.claude/settings.json` — you should see:

```json
{
  "enabledPlugins": {
    "hifz@hifz": true
  }
}
```

### Step 5: Enable auto-context injection (optional but recommended)

By default, hifz captures data but doesn't inject it back into new sessions. To enable automatic context injection at session start, add this to your project's `.claude/settings.local.json` (create the file if it doesn't exist):

```json
{
  "env": {
    "HIFZ_INJECT_CONTEXT": "true"
  }
}
```

With this enabled, every new Claude Code session automatically receives relevant memories and recent observations.

### Step 6: Restart Claude Code

Claude Code reads `.mcp.json` and plugin config on startup. After the steps above, restart Claude Code (or open a new session).

### Step 7: Verify everything works

1. **Check MCP is connected**: Run `/mcp` in Claude Code. `hifz` should show as connected.

2. **Test save**:
   ```
   > use hifz_save to remember that auth uses JWT middleware
   ```

3. **Test recall**:
   ```
   > use hifz_recall to search for authentication
   ```

4. **Test auto-capture**: Use any tool (read a file, run a command), then:
   ```
   > use hifz_timeline to see recent observations
   ```

5. **Test context injection**: Start a new session. You should see saved memories injected automatically at the start (if you enabled `HIFZ_INJECT_CONTEXT`).

## What happens automatically

Once setup is complete, you don't need to do anything manually:

| Feature | How it works |
|---------|-------------|
| **Auto-capture** | Plugin hooks record every tool use, prompt, session start/end to the REST server |
| **Auto-inject** | On session start, relevant memories and recent observations are injected into context (requires `HIFZ_INJECT_CONTEXT=true`) |
| **Manual save/recall** | Use MCP tools (`hifz_save`, `hifz_recall`, `hifz_search`) or slash commands (`/recall`, `/remember`) anytime |

## Storage modes

| Mode | Command | Data | Use case |
|------|---------|------|----------|
| **In-memory** | `hifz serve --memory` | Lost on restart | Quick testing |
| **SurrealKV** | `hifz serve --db-path ~/.hifz/data` | Persistent on disk | Production use |

## MCP Tools

| Tool | Description |
|------|-------------|
| `hifz_recall` | Search observations and memories |
| `hifz_save` | Save insight/pattern/fact |
| `hifz_search` | Hybrid BM25 + vector search |
| `hifz_sessions` | List recent sessions |
| `hifz_digest` | Project intelligence (top concepts, files) |
| `hifz_timeline` | Chronological observations |
| `hifz_export` | Export all data |
| `hifz_delete` | Delete a memory |

## Slash commands (from plugin)

| Command | Description |
|---------|-------------|
| `/recall [query]` | Search hifz for past observations and learnings |
| `/remember [what]` | Save an insight or decision to long-term memory |
| `/forget [what]` | Delete specific observations or memories |
| `/session-history` | Show what happened in recent past sessions |

## REST endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/hifz/health` | Health check |
| POST | `/hifz/observe` | Capture observation |
| POST | `/hifz/session/start` | Start session |
| POST | `/hifz/session/end` | End session |
| POST | `/hifz/smart-search` | Hybrid search |
| POST | `/hifz/remember` | Save memory |
| POST | `/hifz/forget` | Delete memory |
| POST | `/hifz/context` | Generate context |
| GET | `/hifz/digest` | Project intelligence |
| POST | `/hifz/forget-gc` | Garbage collection |
| POST | `/hifz/consolidate` | Run consolidation |
| GET | `/hifz/timeline` | Observation timeline |
| GET | `/hifz/export` | Export all data |

## Configuration

Create `~/.hifz/.env` (optional):

```env
# Ollama for LLM features (compression, consolidation)
# OLLAMA_URL=http://localhost:11434
# OLLAMA_MODEL=qwen2.5:7b

# Search tuning
# TOKEN_BUDGET=2000
```

## Troubleshooting

### MCP tools return errors

The REST server isn't running. Start it:

```bash
./target/release/hifz serve --db-path ~/.hifz/data
```

Verify:

```bash
curl http://localhost:3111/hifz/health
```

### hifz not showing in `/mcp`

Claude Code hasn't picked up the `.mcp.json`. Restart Claude Code.

### Plugin not working (no auto-capture)

Check if the plugin is installed:

```bash
grep -A2 enabledPlugins ~/.claude/settings.json
```

You should see `"hifz@hifz": true`. If not, reinstall:

```
/plugin marketplace add /path/to/hifz/plugin
/plugin install hifz@hifz
```

Then restart Claude Code.

### Context not injected on session start

Make sure `HIFZ_INJECT_CONTEXT` is set to `"true"` in `.claude/settings.local.json`:

```json
{
  "env": {
    "HIFZ_INJECT_CONTEXT": "true"
  }
}
```

### Test MCP binary manually

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | /path/to/hifz mcp
```

Should print JSON with `protocolVersion` and `serverInfo`.

### Test hooks manually

```bash
echo '{"session_id":"test","cwd":"/tmp","tool_name":"Read","tool_input":{"file_path":"src/main.rs"}}' | node plugin/scripts/post-tool-use.mjs
```

### Rebuild after code changes

```bash
cargo build --release
# Restart the REST server (Ctrl+C and rerun)
# Restart Claude Code (to pick up new MCP binary)
```

## Benchmarks

```bash
./benchmark/download_dataset.sh
cargo run --bin longmemeval-bench -- bm25
cargo run --bin longmemeval-bench -- hybrid
```

## License

Apache License 2.0
