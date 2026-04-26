#!/usr/bin/env node
//#region src/hooks/post-tool-failure.ts
const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };

async function main() {
	let input = "";
	for await (const chunk of process.stdin) input += chunk;
	let data;
	try {
		data = JSON.parse(input);
	} catch {
		return;
	}
	if (data.is_interrupt) return;

	const sessionId = data.session_id || "unknown";
	const errorStr = typeof data.error === "string"
		? data.error.slice(0, 4000)
		: JSON.stringify(data.error ?? "").slice(0, 4000);

	// 1. Write: capture the failure
	try {
		await fetch(`${REST_URL}/api/v1/agent/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "post_tool_failure",
				sessionId,
				project: data.cwd || process.cwd(),
				cwd: data.cwd || process.cwd(),
				timestamp: new Date().toISOString(),
				data: {
					tool_name: data.tool_name,
					tool_input: typeof data.tool_input === "string"
						? data.tool_input.slice(0, 4000)
						: JSON.stringify(data.tool_input ?? "").slice(0, 4000),
					error: errorStr
				}
			}),
			signal: AbortSignal.timeout(3000)
		});
	} catch {}

	// 2. Read: search for similar past failures and inject fix context
	if (!errorStr || errorStr.length < 5) return;
	const query = `${data.tool_name || ""} ${errorStr.slice(0, 200)}`.trim();

	try {
		const res = await fetch(`${REST_URL}/api/v1/search/agentic`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({ query, limit: 3, sessionId }),
			signal: AbortSignal.timeout(2000)
		});
		if (!res.ok) return;
		const result = await res.json();
		if (!result.results || result.results.length === 0) return;

		const lines = result.results
			.filter(r => r.score > 0.1)
			.map(r => {
				const type = r.obs_type || "unknown";
				const title = r.title || "";
				const text = r.narrative || "";
				return `- [${type}] **${title}**: ${text}`;
			});
		if (lines.length === 0) return;

		const output = JSON.stringify({
			hookSpecificOutput: {
				hookEventName: "PostToolUseFailure",
				additionalContext: `# Past similar failures / fixes from hifz\n\n${lines.join("\n")}`
			}
		});
		process.stdout.write(output);
	} catch {}
}
main();

//#endregion
export {};
