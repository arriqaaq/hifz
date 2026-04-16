#!/usr/bin/env node
//#region src/hooks/subagent-start.ts
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
		await fetch(`${REST_URL}/hifz/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "subagent_start",
				sessionId,
				project: data.cwd || process.cwd(),
				cwd: data.cwd || process.cwd(),
				timestamp: (/* @__PURE__ */ new Date()).toISOString(),
				data: {
					agent_id: data.agent_id,
					agent_type: data.agent_type
				}
			}),
			signal: AbortSignal.timeout(2e3)
		});
	} catch {}
}
main();

//#endregion
export {  };
//# sourceMappingURL=subagent-start.mjs.map