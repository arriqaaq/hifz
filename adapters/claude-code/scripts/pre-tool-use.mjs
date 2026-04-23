#!/usr/bin/env node
//#region src/hooks/pre-tool-use.ts
const INJECT_CONTEXT = process.env["HIFZ_INJECT_CONTEXT"] === "true";
const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };

async function main() {
	if (!INJECT_CONTEXT) return;
	let input = "";
	for await (const chunk of process.stdin) input += chunk;
	let data;
	try {
		data = JSON.parse(input);
	} catch {
		return;
	}
	const toolName = data.tool_name;
	if (!toolName) return;
	if (!["Edit", "Write", "Read", "Glob", "Grep"].includes(toolName)) return;

	const toolInput = data.tool_input || {};
	const files = [];
	const fileKeys = toolName === "Grep" ? ["path", "file"] : ["file_path", "path", "file", "pattern"];
	for (const key of fileKeys) {
		const val = toolInput[key];
		if (typeof val === "string" && val.length > 0) files.push(val);
	}
	if (files.length === 0) return;

	const terms = [];
	if (toolName === "Grep" || toolName === "Glob") {
		const pattern = toolInput["pattern"];
		if (typeof pattern === "string" && pattern.length > 0) terms.push(pattern);
	}
	const query = [...files, ...terms].join(" ");

	try {
		const res = await fetch(`${REST_URL}/api/v1/search/agentic`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({ query, limit: 5 }),
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
				hookEventName: "PreToolUse",
				additionalContext: `# Relevant hifz context\n\n${lines.join("\n")}`
			}
		});
		process.stdout.write(output);
	} catch {}
}
main();

//#endregion
export {};
