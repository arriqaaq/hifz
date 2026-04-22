#!/usr/bin/env node
//#region src/hooks/stop.ts
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
	
	// Send to /hifz/observe so Stop hook triggers run-close logic
	try {
		await fetch(`${REST_URL}/hifz/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "Stop",
				session_id: sessionId,
				project: data.cwd || process.cwd(),
				timestamp: new Date().toISOString()
			}),
			signal: AbortSignal.timeout(5e3)
		});
	} catch {}
	
	// Also end the session
	try {
		await fetch(`${REST_URL}/hifz/session/end`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({ sessionId }),
			signal: AbortSignal.timeout(5e3)
		});
	} catch {}
}
main();

//#endregion
export {  };
