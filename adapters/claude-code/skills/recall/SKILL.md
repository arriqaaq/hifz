---
name: recall
description: Search hifz for past observations, sessions, and learnings about a topic. Use when the user says "recall", "remember", "what did we do", or needs context from past sessions.
argument-hint: "[search query]"
user-invocable: true
---

The user wants to recall past context about: $ARGUMENTS

Use the `hifz_recall` MCP tool (provided by the hifz server that this plugin wires up automatically via `.mcp.json`) with the user's query as the `query` argument and `limit: 10`. The tool runs hybrid BM25 + vector + RRF fusion with 1-hop graph expansion across the `mem_link` edges between saved memories, then re-ranks by `strength · exp(-age/30) · access_boost`.

Present the returned results to the user in a readable format:
- Group by session
- For each observation show its type, title, and narrative
- Highlight the most important observations (importance >= 7)
- If no results come back, suggest 2-3 alternative search terms the user could try

**Do NOT make up or hallucinate observations.** Only present what the MCP tool actually returned. If `hifz_recall` isn't available, the stdio MCP shim didn't start — tell the user to:
1. Run `/plugin list` in Claude Code and confirm `hifz` shows as enabled.
2. Restart Claude Code (the plugin's `.mcp.json` is only read on startup).
3. Check `/mcp` to see whether the `hifz` MCP server is connected.
