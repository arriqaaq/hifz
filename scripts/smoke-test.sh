#!/usr/bin/env bash
set -euo pipefail

BASE="${HIFZ_URL:-http://localhost:3111}"
PASS=0
FAIL=0
SESSION_ID="smoke_$(date +%s)"

green() { printf "\033[32m  PASS\033[0m %s\n" "$1"; PASS=$((PASS+1)); }
red()   { printf "\033[31m  FAIL\033[0m %s — %s\n" "$1" "$2"; FAIL=$((FAIL+1)); }

check() {
  local name="$1" expected="$2" actual="$3"
  if echo "$actual" | grep -q "$expected"; then
    green "$name"
  else
    red "$name" "expected '$expected', got: $(echo "$actual" | head -c 200)"
  fi
}

echo "=== hifz smoke test ($BASE) ==="
echo ""

# --- Health ---
echo "# Health"
resp=$(curl -s --max-time 3 "$BASE/hifz/health" 2>&1)
if [ $? -ne 0 ]; then
  red "GET /hifz/health" "connection refused — is the server running?"
  echo "Aborting."
  exit 1
fi
check "GET /hifz/health" '"status":"healthy"' "$resp"
resp=$(curl -s "$BASE/hifz/livez" 2>&1) && check "GET /hifz/livez" 'ok' "$resp" || red "GET /hifz/livez" "failed"

# --- Session lifecycle ---
echo ""
echo "# Sessions"
resp=$(curl -s -X POST "$BASE/hifz/session/start" \
  -H 'Content-Type: application/json' \
  -d "{\"sessionId\":\"$SESSION_ID\",\"project\":\"smoke-test\",\"cwd\":\"/tmp\"}" 2>&1)
check "POST /hifz/session/start" "\"sessionId\":\"$SESSION_ID\"" "$resp"

resp=$(curl -s "$BASE/hifz/sessions?limit=5" 2>&1)
check "GET /hifz/sessions" "$SESSION_ID" "$resp"

# --- Observe (simulated hooks) ---
echo ""
echo "# Observe (hook simulation)"

# UserPromptSubmit — starts a run
resp=$(curl -s -X POST "$BASE/hifz/observe" \
  -H 'Content-Type: application/json' \
  -d "{\"hookType\":\"prompt_submit\",\"sessionId\":\"$SESSION_ID\",\"project\":\"smoke-test\",\"cwd\":\"/tmp\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"data\":{\"prompt\":\"smoke test prompt\"}}" 2>&1)
check "observe: UserPromptSubmit" '"status":"ok"' "$resp"

# PostToolUse — Read
resp=$(curl -s -X POST "$BASE/hifz/observe" \
  -H 'Content-Type: application/json' \
  -d "{\"hookType\":\"post_tool_use\",\"sessionId\":\"$SESSION_ID\",\"project\":\"smoke-test\",\"cwd\":\"/tmp\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"data\":{\"tool_name\":\"Read\",\"tool_input\":{\"file_path\":\"/tmp/test.rs\"},\"tool_output\":\"fn main() {}\"}}" 2>&1)
check "observe: PostToolUse (Read)" '"status":"ok"' "$resp"

# PostToolUse — Write (file_write obs_type for run grounding)
resp=$(curl -s -X POST "$BASE/hifz/observe" \
  -H 'Content-Type: application/json' \
  -d "{\"hookType\":\"post_tool_use\",\"sessionId\":\"$SESSION_ID\",\"project\":\"smoke-test\",\"cwd\":\"/tmp\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"data\":{\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":\"/tmp/test.rs\",\"content\":\"fn main() { println!(\\\"hello\\\"); }\"},\"tool_output\":\"ok\"}}" 2>&1)
check "observe: PostToolUse (Write)" '"status":"ok"' "$resp"

# PostToolUse — Bash with git commit output (triggers commit detection)
resp=$(curl -s -X POST "$BASE/hifz/observe" \
  -H 'Content-Type: application/json' \
  -d "{\"hookType\":\"post_tool_use\",\"sessionId\":\"$SESSION_ID\",\"project\":\"smoke-test\",\"cwd\":\"/tmp\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"data\":{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"git commit -m 'smoke test commit'\"},\"tool_output\":\"[main abc1234f] smoke test commit\n 1 file changed, 1 insertion(+)\"}}" 2>&1)
check "observe: PostToolUse (Bash git commit)" '"status":"ok"' "$resp"

# Dedup — same payload should return duplicate
resp=$(curl -s -X POST "$BASE/hifz/observe" \
  -H 'Content-Type: application/json' \
  -d "{\"hookType\":\"post_tool_use\",\"sessionId\":\"$SESSION_ID\",\"project\":\"smoke-test\",\"cwd\":\"/tmp\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"data\":{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"git commit -m 'smoke test commit'\"},\"tool_output\":\"[main abc1234f] smoke test commit\n 1 file changed, 1 insertion(+)\"}}" 2>&1)
check "observe: dedup" '"status":"duplicate"' "$resp"

# Stop — closes run
resp=$(curl -s -X POST "$BASE/hifz/observe" \
  -H 'Content-Type: application/json' \
  -d "{\"hookType\":\"stop\",\"sessionId\":\"$SESSION_ID\",\"project\":\"smoke-test\",\"cwd\":\"/tmp\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"data\":{}}" 2>&1)
check "observe: Stop" '"status"' "$resp"

# --- Search ---
echo ""
echo "# Search"

resp=$(curl -s -X POST "$BASE/hifz/smart-search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"smoke test","limit":3,"project":"smoke-test"}' 2>&1)
check "POST /hifz/smart-search" '"results"' "$resp"

resp=$(curl -s -X POST "$BASE/hifz/search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"smoke test","limit":3}' 2>&1)
check "POST /hifz/search (alias)" '"results"' "$resp"

# --- Remember / Forget ---
echo ""
echo "# Remember & Forget"

resp=$(curl -s -X POST "$BASE/hifz/remember" \
  -H 'Content-Type: application/json' \
  -d '{"title":"smoke test memory","content":"This is a test memory","type":"fact","project":"smoke-test","concepts":["test"]}' 2>&1)
check "POST /hifz/remember" '"status":"ok"' "$resp"

sleep 1
resp=$(curl -s -X POST "$BASE/hifz/smart-search" \
  -H 'Content-Type: application/json' \
  -d '{"query":"smoke test memory fact","limit":5,"project":"smoke-test"}' 2>&1)
check "search finds saved memory" 'smoke test memory' "$resp"

# Extract the memory id for forget
mem_id=$(echo "$resp" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for r in data.get('results', []):
    if r.get('obs_type','').startswith('memory:'):
        rid = r.get('id', {})
        print(f\"{rid['table']}:{rid['key']['String']}\")
        break
" 2>/dev/null || echo "")

if [ -n "$mem_id" ]; then
  resp=$(curl -s -X POST "$BASE/hifz/forget" \
    -H 'Content-Type: application/json' \
    -d "{\"id\":\"$mem_id\"}" 2>&1)
  check "POST /hifz/forget" '"status":"ok"' "$resp"
else
  red "POST /hifz/forget" "could not extract memory id from search"
fi

# --- Context ---
echo ""
echo "# Context"

resp=$(curl -s -X POST "$BASE/hifz/context" \
  -H 'Content-Type: application/json' \
  -d '{"project":"smoke-test","token_budget":2000}' 2>&1)
check "POST /hifz/context" 'context' "$resp"

# --- Core memory ---
echo ""
echo "# Core memory"

resp=$(curl -s "$BASE/hifz/core?project=smoke-test" 2>&1)
check "GET /hifz/core" '"project":"smoke-test"' "$resp"

resp=$(curl -s -X POST "$BASE/hifz/core/edit" \
  -H 'Content-Type: application/json' \
  -d '{"project":"smoke-test","field":"identity","op":"set","value":"smoke test project"}' 2>&1)
check "POST /hifz/core/edit" '"identity":"smoke test project"' "$resp"

# --- Runs ---
echo ""
echo "# Runs"

resp=$(curl -s -X POST "$BASE/hifz/runs" \
  -H 'Content-Type: application/json' \
  -d '{"query":"smoke","project":"smoke-test","limit":5}' 2>&1)
check "POST /hifz/runs" 'runs' "$resp"

# --- Commits ---
echo ""
echo "# Commits"

resp=$(curl -s "$BASE/hifz/commits?project=smoke-test" 2>&1)
check "GET /hifz/commits" '"commits"' "$resp"

# --- Digest ---
echo ""
echo "# Digest & Timeline"

resp=$(curl -s "$BASE/hifz/digest?project=smoke-test" 2>&1)
check "GET /hifz/digest (responds)" '"' "$resp"

resp=$(curl -s "$BASE/hifz/timeline?limit=5" 2>&1)
check "GET /hifz/timeline" 'observations' "$resp"

# --- Export ---
echo ""
echo "# Export"

resp=$(curl -s "$BASE/hifz/export?project=smoke-test" 2>&1)
check "GET /hifz/export" 'observations' "$resp"

# --- Session end ---
echo ""
echo "# Session end"

resp=$(curl -s -X POST "$BASE/hifz/session/end" \
  -H 'Content-Type: application/json' \
  -d "{\"sessionId\":\"$SESSION_ID\"}" 2>&1)
check "POST /hifz/session/end" '"status":"ok"' "$resp"

# --- Consolidation (fires but may produce 0 results with little data) ---
echo ""
echo "# Consolidation"

resp=$(curl -s -X POST "$BASE/hifz/consolidate" \
  -H 'Content-Type: application/json' \
  -d '{}' 2>&1)
check "POST /hifz/consolidate" 'semantic_facts_created\|status\|decayed' "$resp"

# --- Summary ---
echo ""
echo "================================"
printf "  \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n" "$PASS" "$FAIL"
echo "================================"

[ "$FAIL" -eq 0 ] && exit 0 || exit 1
