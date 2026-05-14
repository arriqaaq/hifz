#!/usr/bin/env node
//#region src/hooks/subagent-start.ts
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
	const agentId = data.agent_id;
	try {
		const res = await fetch(`${REST_URL}/api/v1/agent/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "subagent_start",
				sessionId,
				project: data.cwd || process.cwd(),
				cwd: data.cwd || process.cwd(),
				timestamp: (/* @__PURE__ */ new Date()).toISOString(),
				data: {
					agent_id: agentId,
					agent_type: data.agent_type
				}
			}),
			signal: AbortSignal.timeout(2e3)
		});
		if (res.ok && agentId) {
			const body = await res.json().catch(() => null);
			if (body && body.obs_id) {
				await cacheObsId(sessionId, `subagent:${agentId}`, body.obs_id);
			}
		}
	} catch {}
}
main();

//#endregion
export {  };
