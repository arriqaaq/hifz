#!/usr/bin/env node
//#region src/hooks/pre-compact.ts
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
	const project = data.cwd || process.cwd();
	try {
		const res = await fetch(`${REST_URL}/api/v1/context`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				project,
				token_budget: 1500
			}),
			signal: AbortSignal.timeout(5e3)
		});
		if (res.ok) {
			const result = await res.json();
			if (result.context) process.stdout.write(result.context);
		}
	} catch {}
}
main();

//#endregion
export {  };
