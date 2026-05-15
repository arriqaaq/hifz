#!/usr/bin/env node
//#region src/hooks/session-end.ts
import { ingestTranscript } from "./ingest-current-transcript.mjs";
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

	// Belt-and-suspenders: catch any final turn the Stop hook missed
	// (e.g., if the user /exit'd before Stop fired).
	try {
		await ingestTranscript({
			session_id: sessionId,
			transcript_path: data.transcript_path,
			cwd: data.cwd || process.cwd(),
		});
	} catch {}

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
