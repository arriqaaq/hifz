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

## Always-on server (launchd)

The hifz REST server runs as a macOS launchd LaunchAgent (`com.hifz.server`, db `~/.hifz/data`, port 3111) — **not** via `make dev`. Install/manage it with `make install-service` / `restart-service` / `service-status` / `uninstall-service`.

**Whenever hifz Rust code changes, the daemon keeps running the OLD `target/release/hifz` until restarted.** After completing code changes, run `make restart-service` (it does `cargo build --release` then `launchctl kickstart -k`) so the new build is reflected. Do this **once after all phases/builds complete**, not per edit — consistent with the "run builds after all phases" convention.

`make dev` is foreground/testing only and conflicts with the service on port 3111 — stop the service or use `HIFZ_PORT=3120 make dev`. `make stop` only pauses the daemon (KeepAlive resurrects it in ~10s); use `make uninstall-service` to truly stop it.

### Prerequisites
The launchd service must be installed and running (`make install-service`). For ad-hoc/testing use, `cargo run -- serve --db-path ~/.hifz/data` or `--memory`.
