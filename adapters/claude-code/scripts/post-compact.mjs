#!/usr/bin/env node
//#region src/hooks/post-compact.ts
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
	try {
		await fetch(`${REST_URL}/api/v1/agent/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "post_compact",
				sessionId: data.session_id || "unknown",
				project: data.cwd || process.cwd(),
				cwd: data.cwd || process.cwd(),
				timestamp: new Date().toISOString(),
				data: {
					trigger: data.trigger,
					custom_instructions: data.custom_instructions,
					summary: data.summary
				}
			}),
			signal: AbortSignal.timeout(3000)
		});
	} catch {}
}
main();

//#endregion
export {};
