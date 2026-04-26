#!/usr/bin/env node
//#region src/hooks/prompt-submit.ts
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
	const sessionId = data.session_id || "unknown";
	const prompt = data.prompt || "";

	// 1. Write: capture the prompt as an observation
	try {
		await fetch(`${REST_URL}/api/v1/agent/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "prompt_submit",
				sessionId,
				project: data.cwd || process.cwd(),
				cwd: data.cwd || process.cwd(),
				timestamp: new Date().toISOString(),
				data: { prompt }
			}),
			signal: AbortSignal.timeout(3000)
		});
	} catch {}

	// 2. Read: search hifz for context relevant to this prompt
	if (!prompt || prompt.length < 10) return; // skip trivial prompts
	try {
		const res = await fetch(`${REST_URL}/api/v1/search/agentic`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({ query: prompt, limit: 5, sessionId }),
			signal: AbortSignal.timeout(3000)
		});
		if (!res.ok) return;
		const results = await res.json();
		if (!results.results || results.results.length === 0) return;

		// Format results as context
		const lines = results.results
			.filter(r => r.score > 0.1)
			.map(r => {
				const type = r.obs_type || "unknown";
				const title = r.title || "";
				const text = r.narrative || "";
				return `- [${type}] **${title}**: ${text}`;
			});

		if (lines.length === 0) return;

		const context = `# Relevant hifz context\n\n${lines.join("\n")}`;

		// Return as additionalContext so Claude Code injects it
		const output = JSON.stringify({
			hookSpecificOutput: {
				hookEventName: "UserPromptSubmit",
				additionalContext: context
			}
		});
		process.stdout.write(output);
	} catch {}
}
main();

//#endregion
export {};
//# sourceMappingURL=prompt-submit.mjs.map
