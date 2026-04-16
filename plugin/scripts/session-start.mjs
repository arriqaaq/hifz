#!/usr/bin/env node
//#region src/hooks/session-start.ts
const INJECT_CONTEXT = process.env["HIFZ_INJECT_CONTEXT"] === "true";
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
	const sessionId = data.session_id || `ses_${Date.now().toString(36)}`;
	const project = data.cwd || process.cwd();
	try {
		const res = await fetch(`${REST_URL}/hifz/session/start`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				sessionId,
				project,
				cwd: project
			}),
			signal: AbortSignal.timeout(5e3)
		});
		if (INJECT_CONTEXT && res.ok) {
			const result = await res.json();
			if (result.context) process.stdout.write(result.context);
		}
	} catch {}
}
main();

//#endregion
export {  };
//# sourceMappingURL=session-start.mjs.map