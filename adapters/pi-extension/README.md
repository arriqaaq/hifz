# `@hifz/pi-extension`

Pi extension that captures every Pi event into a local Hifz instance.

It does two things at once:

- **Lossless ledger.** Every Pi hook event POSTs to `/api/v1/agent/events` (no embedding, no compression). Replayable and cheap.
- **Semantic memory.** A curated subset of events also POSTs to `/api/v1/agent/observe`, `/api/v1/memories`, and `/api/v1/consolidate` so Pi sessions show up in Hifz's `observation` / `run` / `memory` graph alongside Claude Code data.

See `/Users/kfarhan/.claude/plans/can-you-create-a-gleaming-manatee.md` for the full design.

---

## Full setup (run Pi with Ollama + Hifz)

End-to-end runbook. Six terminals total but most are one-time setup.

### Terminal 1 — Ollama daemon (one-time install, then leave running)

```bash
# Install once if you haven't:
brew install ollama

# Start the daemon
ollama serve
# Leave running.
```

### Terminal 2 — Pull a tool-capable model (one-time)

```bash
ollama pull qwen2.5-coder:7b
ollama list                  # verify
```

Other tool-capable options: `qwen3:8b`, `llama3.1:8b`, `gpt-oss:20b`, `qwen2.5-coder:32b` (heavier).

### Terminal 3 — Configure Pi to use Ollama (one-time)

```bash
mkdir -p ~/.pi/agent
cat > ~/.pi/agent/models.json <<'EOF'
{
  "providers": {
    "ollama": {
      "baseUrl": "http://localhost:11434/v1",
      "api": "openai-completions",
      "apiKey": "ollama",
      "compat": {
        "supportsDeveloperRole": false,
        "supportsReasoningEffort": false
      },
      "models": [
        { "id": "qwen2.5-coder:7b", "name": "Qwen 2.5 Coder 7B (local)" }
      ]
    }
  }
}
EOF
```

Notes:
- `apiKey: "ollama"` is a placeholder; Ollama ignores it but Pi requires the field.
- The two `compat: false` flags are required for OpenAI-compatible servers — without them Pi sends fields Ollama can't parse.
- Drop `reasoning: true` on models that don't actually emit reasoning blocks.

Install Pi if you haven't:

```bash
npm install -g @mariozechner/pi-coding-agent
pi --version
```

### Terminal 4 — Install the Hifz extension (one-time)

```bash
cd /path/to/hifz/adapters/pi-extension
npm install                                            # pulls Pi types
mkdir -p ~/.pi/agent/extensions
ln -sfn "$(pwd)" ~/.pi/agent/extensions/hifz-logger
ls -l ~/.pi/agent/extensions/hifz-logger                     # confirm symlink
```

No build step — Pi loads `index.ts` via jiti at runtime.

### Terminal 5 — Start Hifz

```bash
cd /path/to/hifz

# Optional: wipe any half-migrated DB from a previous failed start:
rm -rf ~/.hifz/data

cargo run -- serve --db-path ~/.hifz/data
```

You should see `Database schema initialized` and the server listening on `:3111`. **Leave running.**

Sanity check from another shell:

```bash
curl -s -X POST http://localhost:3111/api/v1/agent/events \
  -H 'content-type: application/json' \
  -d '{"source":"manual","event_type":"ping","timestamp":"2026-05-01T00:00:00Z","payload_hash":"manual-1","payload":{"hi":1}}'

curl -s 'http://localhost:3111/api/v1/agent/events?source=manual' | jq
```

If these return non-2xx, stop and check the Hifz log before launching Pi.

### Terminal 6 — Run Pi

```bash
export HIFZ_URL=http://localhost:3111
pi
```

In Pi's UI:

1. The startup banner should list **`hifz-logger`** under loaded extensions. If it doesn't, run `pi -e ~/.pi/agent/extensions/hifz-logger/index.ts` to surface the load error.
2. Type `/model` and pick `ollama / qwen2.5-coder:7b`.
3. Type a prompt, e.g. `read package.json and tell me what version this is`.
4. Wait for it to finish.
5. `/exit`.

---

## Verify capture

```bash
HIFZ=http://localhost:3111

# A. Lossless ledger — every event is here
curl -s "$HIFZ/api/v1/agent/events?source=pi_extension&limit=20" \
  | jq '.count, (.events[0:5] | map(.event_type))'

# B. Promoted events became observations
curl -s "$HIFZ/api/v1/agent/observations?obs_type=user_prompt"  | jq '.count'
curl -s "$HIFZ/api/v1/agent/observations?obs_type=file_read"    | jq '.count'

# C. Run created and linked
curl -s -X POST "$HIFZ/api/v1/agent/runs" \
  -H 'content-type: application/json' \
  -d '{"query":"package.json","limit":3}' | jq

# D. Semantic recall over Pi data
curl -s -X POST "$HIFZ/api/v1/search/session" \
  -H 'content-type: application/json' \
  -d '{"query":"what files have we read","limit":5}' | jq '.results[]?.title'

# E. Digest pulls Pi entities
curl -s "$HIFZ/api/v1/agent/digest" | jq '.files[0:5], .keywords[0:10]'
```

Expected outcomes:

- A: tens of events for one prompt (no per-token deltas).
- B: at least one `user_prompt` and one `file_read` observation.
- C: a run row with `observation_ids` populated.
- D: returns the Pi-derived `file_read` observation about `package.json` via embedding similarity.
- E: `package.json` appears in `files`.

---

## Knowledge-enrichment paths

These exercise the Claude-Code-parity enrichment passes:

```text
# Inside Pi:
"create a tiny test file and commit it"
```

```bash
curl -s "$HIFZ/api/v1/agent/observations?obs_type=commit_made" | jq
# → observation with metadata.sha, branch, files
```

```text
# Inside Pi:
"write a plan file at .pi/plans/test.md with H1 'Test Plan' and H2 sections"
```

```bash
curl -s "$HIFZ/api/v1/memories?category=plan" | jq
# → pinned memory with title, sections, file refs
```

---

## Configuration (env vars)

| Var | Default | Meaning |
|---|---|---|
| `HIFZ_URL` | `http://localhost:3111` | Hifz REST URL. Empty string disables the extension. |
| `HIFZ_SPOOL` | `~/.hifz/spool/pi-extension` | Disk fallback when Hifz is unreachable. |
| `HIFZ_PLAN_GLOB` | (none) | Extra regex matched against write-tool paths to identify plan files. Defaults already include `.claude/plans/*.md`, `.pi/plans/*.md`, `.cursor/plans/*.md`. |
| `HIFZ_VERBOSE_DELTAS` | `0` | Set to `1` to also store per-token streaming deltas (large volume; debug only). |

---

## What gets stored per Pi turn

- ~tens of `event` rows: prompt, tool execution, message, etc. (deltas dropped by default).
- ~5–15 `observation` rows: the prompt, each tool result with a sensible `obs_type` (`file_read`, `file_write`, `file_edit`, `command_run`, `search`, `error`), the final assistant message, any compaction summary.
- One `run` row tying the prompt to its observations and outcome.
- A `commit_made` observation if the turn ran `git commit`.
- A pinned `category: "plan"` memory if the turn wrote to a plan path.
- A consolidation pass at session end.

---

## Offline replay

```bash
# While Pi is running:
# 1. Stop Hifz (Ctrl-C in Terminal 5). Pi keeps running.
# 2. Continue using Pi. Spool grows on disk:
ls ~/.hifz/spool/pi-extension/
# 3. Restart Hifz. Spool drains automatically on the next event.
ls ~/.hifz/spool/pi-extension/    # should empty within seconds
```

---

## Disabling

```bash
rm ~/.pi/agent/extensions/hifz-logger    # extension off
unset HIFZ_URL                     # alternative: extension no-ops with empty URL
```

---

## Troubleshooting

| Symptom | Likely cause / fix |
|---|---|
| Extension not in Pi banner | Symlink wrong, or `index.ts` failed to load → run `pi -e <path>/index.ts` for the error. |
| `fetch failed` in Pi log | Hifz not running, or `HIFZ_URL` mismatch. |
| `model does not support tools` | Pick a tool-capable Ollama model (qwen2.5-coder, qwen3, llama3.1). Older `mistral`, `gemma`, `phi` variants often lack tools. |
| Pi sends `developer` role and Ollama 400s | Ensure `compat.supportsDeveloperRole=false` is in the provider block. |
| Pi sends `reasoning_effort` and Ollama 400s | Same fix — `compat.supportsReasoningEffort=false`. |
| `connection refused localhost:11434` | `ollama serve` isn't running. |
| Model is slow on first prompt | First call loads weights. Pre-warm with `ollama run qwen2.5-coder:7b "hi"` once. |
| Tool calls hallucinate or loop | Local 7B models are weak at tools. Try qwen2.5-coder:32b or gpt-oss:20b if you have ≥32 GB RAM. |
| No `observation`s, only `event`s | Promotion filter dropped them — confirm Pi tools are named `read`/`write`/`edit`/`bash` (case-insensitive); otherwise update `tool_to_obs_type` in `src/promote.ts`. |
| `UNIQUE` violation on retried event | Expected — that's the idempotency check; the handler returns 200 with the existing id. |

---

## TL;DR

```bash
# Ollama
brew install ollama && ollama serve &
ollama pull qwen2.5-coder:7b

# Pi config
mkdir -p ~/.pi/agent && cat > ~/.pi/agent/models.json <<'EOF'
{ "providers": { "ollama": {
    "baseUrl": "http://localhost:11434/v1", "api": "openai-completions", "apiKey": "ollama",
    "compat": { "supportsDeveloperRole": false, "supportsReasoningEffort": false },
    "models": [ { "id": "qwen2.5-coder:7b" } ]
} } }
EOF

# Hifz extension
cd /path/to/hifz/adapters/pi-extension && npm install
ln -sfn "$(pwd)" ~/.pi/agent/extensions/hifz-logger

# Run
( cd /path/to/hifz && cargo run -- serve --db-path ~/.hifz/data ) &
HIFZ_URL=http://localhost:3111 pi
# /model → ollama/qwen2.5-coder:7b → type a prompt
```
