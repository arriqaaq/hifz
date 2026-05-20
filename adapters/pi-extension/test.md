# Pi extension — end-to-end test script

Concrete test cases for verifying the `@hifz/pi-extension` dual-write pipeline. Each test has a Pi prompt to type and curl commands to verify Hifz captured what it should.

Prerequisites:
- Hifz running on `http://localhost:3111`
- Pi running with the extension symlinked into `~/.pi/extensions/hifz-logger`
- `HIFZ_URL=http://localhost:3111` exported for Pi

```bash
HIFZ=http://localhost:3111
```

## Test 1 — Basic prompt + file read

In Pi:
```
read AGENTS.md and tell me the project's name
```

Then:
```bash
# Lossless ledger — should be ~10–30 events for one prompt
curl -s "$HIFZ/api/v1/agent/events?source=pi_extension&limit=200" \
  | jq '.count, (.events | map(.event_type) | group_by(.) | map({type: .[0], n: length}))'

# User prompt as a searchable observation
curl -s "$HIFZ/api/v1/agent/observations?obs_type=user_prompt" \
  | jq '.count, .observations[].title // .observations[].narrative'

# File read promoted
curl -s "$HIFZ/api/v1/agent/observations?obs_type=file_read" \
  | jq '.count, (.observations | map({title, files}))'

# A Hifz run was opened by UserPromptSubmit
curl -s -X POST "$HIFZ/api/v1/agent/runs" -H 'content-type: application/json' \
  -d '{"query":"AGENTS","limit":3}' \
  | jq '.runs[]? | {prompt, outcome, observations: (.observation_ids|length)}'
```

Expect: count > 0 for each, run shows the prompt with `observation_ids` non-empty.

## Test 2 — File write + plan capture

In Pi:
```
create a file at .pi/plans/test-plan.md with the following markdown:
# Hifz Test Plan
## Goals
verify events table works
## Files
src/db.rs and src/web/api.rs
```

Then:
```bash
# Write tool ended → file_write observation
curl -s "$HIFZ/api/v1/agent/observations?obs_type=file_write" \
  | jq '.observations[] | {title, files}'

# Plan path matched → memory POSTed (plans.ts enrichment)
curl -s "$HIFZ/api/v1/memories?category=plan" \
  | jq '.memories[]? | {title, files, keywords}'
```

Expect: a `file_write` observation for `test-plan.md`, and a memory titled `Hifz Test Plan` with `src/db.rs` and `src/web/api.rs` extracted into `files`.

## Test 3 — Bash command + git commit enrichment

In Pi (run as a single message):
```
run `echo hello > /tmp/hifz-test.txt` then run `cd /tmp && git init && git add hifz-test.txt && git commit -m "test: hifz commit detection"`
```

Then:
```bash
# Plain bash → command_run observation
curl -s "$HIFZ/api/v1/agent/observations?obs_type=command_run" \
  | jq '.observations[].title' | head -10

# git commit → commit_made observation with sha + files (git.ts enrichment)
curl -s "$HIFZ/api/v1/agent/observations?obs_type=commit_made" \
  | jq '.observations[]? | {title, sha: .metadata.sha, branch: .metadata.branch, files: .metadata.files}'
```

Expect: `command_run` rows for the echo and git steps, **plus** a `commit_made` row with a real SHA, branch (`main`/`master`), and `files: ["hifz-test.txt"]`.

## Test 4 — Tool error capture

In Pi:
```
read /nonexistent/file.txt
```

Then:
```bash
curl -s "$HIFZ/api/v1/agent/observations?obs_type=error" \
  | jq '.observations[] | {title, narrative: (.narrative // "" | .[0:100])}'
```

Expect: an `error` observation referencing the missing file.

## Test 5 — Semantic recall (proves the embedding pipeline reached fastembed)

```bash
curl -s -X POST "$HIFZ/api/v1/search/session" -H 'content-type: application/json' \
  -d '{"query":"agents.md project name","limit":5}' \
  | jq '.results[]? | {title, obs_type, score}'

curl -s -X POST "$HIFZ/api/v1/search/session" -H 'content-type: application/json' \
  -d '{"query":"git commit detection test","limit":5}' \
  | jq '.results[]? | {title, obs_type}'

curl -s -X POST "$HIFZ/api/v1/search/session" -H 'content-type: application/json' \
  -d '{"query":"what files have we written","limit":10}' \
  | jq '.results[]? | {title, obs_type, files}'
```

Expect: each query returns relevant rows from the prior tests via embedding similarity. The third query should surface the `file_write` observation for `test-plan.md`.

## Test 6 — Digest (proves entity extraction)

```bash
curl -s "$HIFZ/api/v1/agent/digest" \
  | jq '{files: .files[0:10], keywords: .keywords[0:10], commits: .commits}'
```

Expect: `files` includes `AGENTS.md`, `test-plan.md`, `src/db.rs`, `src/web/api.rs`; `keywords` contains tokens from the prompts.

## Test 7 — Replay one Pi turn from the ledger

```bash
# Pull the latest Pi session id
SID=$(curl -s "$HIFZ/api/v1/agent/sessions?limit=1" | jq -r '.sessions[0].id // .sessions[0].sessionId')
echo "session: $SID"

# Walk every event in that session
curl -s "$HIFZ/api/v1/agent/events?source=pi_extension&session_id=$SID&limit=500" \
  | jq '.events | sort_by(.sequence) | map({seq: .sequence, type: .event_type, t: .timestamp})'
```

Expect: a chronologically ordered trace of what happened in the most recent Pi session — `agent_start → turn_start → input → before_provider_request → tool_execution_start → tool_execution_end → message_end → turn_end → agent_end`.

## Test 8 — Idempotency

```bash
# Try to re-POST one of the events Pi already sent.
LAST_HASH=$(curl -s "$HIFZ/api/v1/agent/events?source=pi_extension&limit=1" | jq -r '.events[0].payload_hash')
echo "hash: $LAST_HASH"

curl -s -X POST "$HIFZ/api/v1/agent/events" -H 'content-type: application/json' \
  -d "{\"source\":\"pi_extension\",\"event_type\":\"replay\",\"timestamp\":\"2026-05-01T00:00:00Z\",\"payload_hash\":\"$LAST_HASH\",\"payload\":{}}" \
  | jq
```

Expect: `{"status":"duplicate","id":...}` — the UNIQUE index returns the existing row instead of inserting.

---

## Quick pass/fail sweep

If you want one block to confirm everything at once:

```bash
HIFZ=http://localhost:3111
for q in user_prompt file_read file_write file_edit command_run commit_made error compaction_summary; do
  n=$(curl -s "$HIFZ/api/v1/agent/observations?obs_type=$q" | jq -r '.count // 0')
  echo "$q: $n"
done

echo "---"
echo "events: $(curl -s "$HIFZ/api/v1/agent/events?source=pi_extension&limit=1000" | jq '.count')"
echo "memories(plan): $(curl -s "$HIFZ/api/v1/memories?category=plan" | jq '.memories|length')"
echo "runs: $(curl -s -X POST "$HIFZ/api/v1/agent/runs" -H 'content-type: application/json' -d '{"query":"*","limit":50}' | jq '.runs|length')"
```

Expected after running tests 1–4: `user_prompt: ≥4`, `file_read: ≥1`, `file_write: ≥1`, `command_run: ≥1`, `commit_made: ≥1`, `error: ≥1`, `events: 50+`, `memories(plan): ≥1`, `runs: ≥4`.

Paste the sweep output back if any line shows `0` and we'll diagnose just that one.
