#!/usr/bin/env node
//#region src/hooks/session-start.ts
const INJECT_CONTEXT = process.env["HIFZ_INJECT_CONTEXT"] !== "false";
const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };
const WARMUP_TOP_N = Number.parseInt(process.env["HIFZ_WARMUP_TOP_N"] || "15", 10);

/**
 * Format a warmup digest as a human-readable system-context block.
 * Compact-by-design — the agent gets a "here's where you are" snapshot
 * without burning the conversation budget.
 */
function formatWarmup(digest) {
	const lines = [];
	lines.push(`# hifz session warmup — project: ${digest.project}`);
	lines.push("");
	if (digest.latest_plan) {
		lines.push("## Active plan");
		lines.push(`- **${digest.latest_plan.title}** — ${digest.latest_plan.summary}`);
		lines.push("");
	}
	const sections = [
		["Decisions", digest.decisions],
		["Conventions", digest.conventions],
		["Open bugs", digest.open_bugs],
		["Gotchas", digest.gotchas],
		["Failure patterns", digest.failure_patterns],
		["Recent lessons", digest.recent_lessons],
	];
	for (const [name, list] of sections) {
		if (!list || list.length === 0) continue;
		lines.push(`## ${name}`);
		for (const e of list) {
			lines.push(`- **${e.title}** — ${e.summary}`);
		}
		lines.push("");
	}
	return lines.join("\n");
}

async function main() {
	let input = "";
	for await (const chunk of process.stdin) input += chunk;
	let data;
	try {
		data = JSON.parse(input);
	} catch {
		return;
	}
	const sessionId = data.session_id || `ses_${Date.now().toString(36)}`;
	const project = data.cwd || process.cwd();
	try {
		// NOTE: we deliberately do NOT register the session here. The
		// server lazily creates the session row (observe::ensure_session)
		// on the first real observation, so a SessionStart with no
		// follow-up activity (startup/resume/clear/compact churn) never
		// leaves an empty "ghost" session row.

		// Pull the project-scoped warmup digest and inject as system
		// context. Failure here is silent — warmup is a nice-to-have,
		// not a session-blocker, and is project-scoped (no session row
		// required).
		if (INJECT_CONTEXT) {
			const warmupRes = await fetch(
				`${REST_URL}/api/v1/agent/sessions/${encodeURIComponent(sessionId)}/warmup?project=${encodeURIComponent(project)}&top_n=${WARMUP_TOP_N}`,
				{
					method: "GET",
					headers: HEADERS,
					signal: AbortSignal.timeout(5e3),
				},
			);
			if (warmupRes.ok) {
				const digest = await warmupRes.json();
				if (digest && !digest.error && digest.top && digest.top.length > 0) {
					const block = formatWarmup(digest);
					// Claude Code hooks: stdout is injected as additional
					// context for the upcoming model turn.
					process.stdout.write(block);
				}
			}
		}
	} catch {}
}
main();

//#endregion
export {};
//# sourceMappingURL=session-start.mjs.map
