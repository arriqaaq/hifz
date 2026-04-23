#!/usr/bin/env node
//#region src/hooks/session-end.ts
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
	try {
		await fetch(`${REST_URL}/api/v1/agent/sessions/end`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({ sessionId }),
			signal: AbortSignal.timeout(5e3)
		});
	} catch {}
	if (process.env["CONSOLIDATION_ENABLED"] === "true") {
		try {
			await fetch(`${REST_URL}/api/v1/consolidate`, {
				method: "POST",
				headers: HEADERS,
				body: JSON.stringify({}),
				signal: AbortSignal.timeout(3e4)
			});
		} catch {}
	}
}
main();

//#endregion
export {  };
