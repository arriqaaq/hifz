---
name: remember
description: Explicitly save an insight, decision, or learning to hifz's long-term storage. Use when the user says "remember this", "save this", or wants to preserve knowledge for future sessions.
argument-hint: "[what to remember]"
user-invocable: true
---

The user wants to save this to long-term memory: $ARGUMENTS

Persist it via hifz. **Preferred path: the `hifz save` CLI** (run through Bash) — it
POSTs straight to the REST server and is serialization-proof. Claude Code has a known
arg-drop bug (`anthropics/claude-code#3966`-class) where an MCP `tools/call` can arrive
with empty arguments; the `mcp__hifz__hifz_save` tool is otherwise equivalent (same
`POST /api/v1/memories`). **If the MCP tool returns `-32602` / "Received keys: []", fall
back to `hifz save` — never retry the MCP tool repeatedly and never skip the save.**

**`content` is the only required field. `title` is optional — if you omit it the server derives a headline from the first line of `content`. Never block or skip a save just to invent a title.**

Steps:
1. Analyze what the user wants to remember — pull out the core insight, decision, or fact.
2. Optionally write a short (`<~80` char) `title` headline. Skip it if nothing better than the first line of the content comes to mind — the server will derive one.
3. Extract 2-5 searchable `keywords` (lowercased keyword phrases) that capture what the memory is about. Prefer specific terms over generic ones (`"jwt-refresh-rotation"` beats `"auth"`).
4. Extract any relevant `files` — absolute or repo-relative paths the memory references.
5. Save it. Preferred — the CLI (quote multi-line content with `$'…'` or a heredoc):
   ```
   hifz save --content "<text>" [--title "<t>"] [--project <p>] \
             [--category decision|lesson|gotcha|convention|…] \
             [--keyword k1 --keyword k2] [--file path1 --file path2]
   ```
   Or call the `hifz_save` MCP tool with the same fields (`content` required;
   `title`/`keywords`/`files`/`category`/`project` optional). On `-32602`, switch to
   the CLI form above.
6. Confirm to the user that the memory was saved and show the title (as stored — yours or the derived one) + keywords you tagged so they know what terms will retrieve it later.

If neither path works: the REST server may be down. Tell the user to check
`make service-status` (the always-on daemon on :3111), and that for the MCP tool
specifically `/plugin list` shows `hifz` enabled and `/mcp` shows it connected
(the plugin's `.mcp.json` is only read on Claude Code startup).
