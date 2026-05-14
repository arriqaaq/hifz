#!/usr/bin/env node
//#region src/hooks/subagent-stop.ts
import { promises as fs } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };
const STATE_DIR = join(homedir(), ".hifz", "hook-state");

async function lookupParent(sessionId, key) {
	if (!sessionId || !key) return null;
	try {
		const path = join(STATE_DIR, `${sessionId}.json`);
		const state = JSON.parse(await fs.readFile(path, "utf8"));
		return state[key] || null;
	} catch {
		return null;
	}
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
	const agentId = data.agent_id;
	const lastMsg = typeof data.last_assistant_message === "string" ? data.last_assistant_message.slice(0, 4e3) : "";
	const parentObsId = agentId ? await lookupParent(sessionId, `subagent:${agentId}`) : null;
	try {
		await fetch(`${REST_URL}/api/v1/agent/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "subagent_stop",
				sessionId,
				project: data.cwd || process.cwd(),
				cwd: data.cwd || process.cwd(),
				timestamp: (/* @__PURE__ */ new Date()).toISOString(),
				parentObsId,
				data: {
					agent_id: agentId,
					agent_type: data.agent_type,
					last_message: lastMsg
				}
			}),
			signal: AbortSignal.timeout(2e3)
		});
	} catch {}
}
main();

//#endregion
export {  };
