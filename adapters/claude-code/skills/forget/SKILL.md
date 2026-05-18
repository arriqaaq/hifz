---
name: forget
description: Delete specific observations or sessions from hifz. Use when user says "forget this", "delete memory", or wants to remove specific data for privacy.
argument-hint: "[what to forget - session ID, file path, or search term]"
user-invocable: true
---

The user wants to remove data from hifz: $ARGUMENTS

**IMPORTANT**: This is a destructive operation. Always confirm with the user before deleting.

Steps:

1. First search for matching memories with the `hifz_search` MCP tool (provided by the hifz server this plugin wires up via `.mcp.json`). Use the user's input as the `query` with `limit: 20`.
2. Show the user what was found — session IDs, memory IDs, titles — and ask for explicit confirmation before deleting.
3. Once confirmed, delete each memory by calling `hifz_delete` **once per memory id**:
   - `id` — a single memory id string like `"memory:<key>"` (from the search results in step 1)

   `hifz_delete` removes one memory per call and takes no other arguments (no `reason`, no batch array). To drop several memories — or an entire session's memories — collect every relevant `memory:<key>` from the search results and call `hifz_delete` once for each. There is no bare-`sessionId` delete; deletion is by memory id only.
4. Confirm the number of memories deleted back to the user.

**Never delete without explicit user confirmation.** If the MCP tools aren't available, the stdio MCP shim didn't start — tell the user to:
1. Run `/plugin list` in Claude Code and confirm `hifz` shows as enabled.
2. Restart Claude Code (the plugin's `.mcp.json` is only read on startup).
3. Check `/mcp` to see whether the `hifz` MCP server is connected.
