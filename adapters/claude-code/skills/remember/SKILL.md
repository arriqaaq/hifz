---
name: remember
description: Explicitly save an insight, decision, or learning to hifz's long-term storage. Use when the user says "remember this", "save this", or wants to preserve knowledge for future sessions.
argument-hint: "[what to remember]"
user-invocable: true
---

The user wants to save this to long-term memory: $ARGUMENTS

Use the `hifz_save` MCP tool (provided by the hifz server that this plugin wires up automatically via `.mcp.json`) to persist it.

**`content` is the only required field. `title` is optional — if you omit it the server derives a headline from the first line of `content`. Never block or skip a save just to invent a title.**

Steps:
1. Analyze what the user wants to remember — pull out the core insight, decision, or fact.
2. Optionally write a short (`<~80` char) `title` headline. Skip it if nothing better than the first line of the content comes to mind — the server will derive one.
3. Extract 2-5 searchable `keywords` (lowercased keyword phrases) that capture what the memory is about. Prefer specific terms over generic ones (`"jwt-refresh-rotation"` beats `"auth"`).
4. Extract any relevant `files` — absolute or repo-relative paths the memory references.
5. Call `hifz_save` with the fields:
   - `content` — **REQUIRED**, the full text to remember (preserve the user's phrasing as much as possible)
   - `title` — optional headline; omit to let the server derive it from `content`
   - `keywords` — the extracted concept list
   - `files` — the extracted file list (empty array if none apply)
   - `category` — optional typed bucket (defaults to `note`); use e.g. `decision`, `lesson`, `gotcha`, `convention` when it fits
   - `project` — optional project name (defaults to `global`)
6. Confirm to the user that the memory was saved and show the title (as stored — yours or the derived one) + keywords you tagged so they know what terms will retrieve it later.

If `hifz_save` isn't available, the stdio MCP shim didn't start — tell the user to:
1. Run `/plugin list` in Claude Code and confirm `hifz` shows as enabled.
2. Restart Claude Code (the plugin's `.mcp.json` is only read on startup).
3. Check `/mcp` to see whether the `hifz` MCP server is connected.
