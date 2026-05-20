#!/usr/bin/env node
//#region src/hooks/prompt-submit.ts
import { promises as fs } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };
const STATE_DIR = join(homedir(), ".hifz", "hook-state");

async function cacheObsId(sessionId, key, obsId) {
	if (!sessionId || !key || !obsId) return;
	try {
		await fs.mkdir(STATE_DIR, { recursive: true });
		const path = join(STATE_DIR, `${sessionId}.json`);
		let state = {};
		try {
			state = JSON.parse(await fs.readFile(path, "utf8"));
		} catch {}
		state[key] = obsId;
		await fs.writeFile(path, JSON.stringify(state));
	} catch {}
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
	const sessionId = data.session_id || "unknown";
	const prompt = data.prompt || "";

	// 1. Write: capture the prompt as an observation. Cache its obs_id as
	//    `current_prompt` so subsequent PostToolUse observations within this
	//    prompt window can stamp themselves as children — that's the causal
	//    chain that lets readers reconstruct "what led to what".
	try {
		const res = await fetch(`${REST_URL}/api/v1/agent/observe`, {
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
		if (res.ok) {
			const body = await res.json().catch(() => null);
			if (body && body.obs_id) {
				await cacheObsId(sessionId, "current_prompt", body.obs_id);
			}
		}
	} catch {}

	// 2. Read: search hifz for context relevant to this prompt
	if (!prompt || prompt.length < 10) return; // skip trivial prompts
	try {
		const res = await fetch(`${REST_URL}/api/v1/search/session`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({ query: prompt, limit: 5, sessionId }),
			signal: AbortSignal.timeout(3000)
		});
		if (!res.ok) return;
		const results = await res.json();
		if (!results.results || results.results.length === 0) return;

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
