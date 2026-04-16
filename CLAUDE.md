# hifz — Persistent Memory for Claude Code

## Memory (hifz)

This project has a persistent memory system via the hifz MCP server.

### Auto-recall
At the start of work or when context seems missing, use `hifz_recall` or `hifz_search` to find relevant memories and observations. Do this proactively — don't wait to be asked. Search for terms related to the current task (e.g. file names, concepts, module names).

### Auto-save
When you learn something important during a session, use `hifz_save` to persist it **without asking the user**. Save things like:
- Architectural decisions and patterns
- Bug root causes and fixes
- Workflows and processes
- Non-obvious project conventions
- Important preferences the user expresses

Do NOT save trivial things like "read a file" or "ran a command" — hooks already capture those as observations. Only save insights that would be valuable in a future session.

### Prerequisites
The REST server must be running (`cargo run -- serve --db-path ~/.hifz/data` or `--memory`).
