#!/usr/bin/env node
//#region src/hooks/post-tool-use.ts
import { exec } from "node:child_process";
import { promisify } from "node:util";

const execAsync = promisify(exec);
const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };

const OBS_TYPE_MAP = {
	Read: "file_read",
	Write: "file_write",
	Edit: "file_edit",
	MultiEdit: "file_edit",
	Bash: "command_run",
	Shell: "command_run",
	Grep: "search",
	Glob: "search",
	WebFetch: "web_fetch",
	WebSearch: "web_fetch",
};

function isGitCommitCommand(command) {
	const subs = command.split(/[&;|]/);
	for (const sub of subs) {
		const trimmed = sub.trim().replace(/^&+/, "").trim();
		if (!trimmed) continue;
		const parts = trimmed.split(/\s+/);
		const gitIdx = parts.indexOf("git");
		if (gitIdx === -1) continue;
		let commitIdx = -1;
		for (let i = gitIdx + 1; i < parts.length; i++) {
			if (!parts[i].startsWith("-") || parts[i] === "--") {
				commitIdx = i;
				break;
			}
		}
		if (commitIdx === -1) continue;
		if (parts[commitIdx] !== "commit") continue;
		if (parts.some((p) => p === "--dry-run" || p === "-n")) continue;
		return true;
	}
	return false;
}

function parseCommitOutput(output) {
	const lines = output.split("\n");
	for (const line of lines) {
		const trimmed = line.trim();
		if (!trimmed.startsWith("[")) continue;
		const closeBracket = trimmed.indexOf("]");
		if (closeBracket === -1) continue;
		const inside = trimmed.substring(1, closeBracket);
		const rest = trimmed.substring(closeBracket + 1).trim();
		const parts = inside.split(/\s+/);
		if (parts.length < 2) continue;
		const branch = parts[0];
		const sha = parts[parts.length - 1];
		if (sha.length < 7 || !/^[0-9a-f]+$/i.test(sha)) continue;
		return { sha, branch, message: rest };
	}
	return null;
}

async function getCommitFiles(sha, cwd) {
	try {
		const { stdout } = await execAsync(
			`git diff-tree --no-commit-id -r --name-only ${sha}`,
			{ cwd, timeout: 5000 }
		);
		return stdout.trim().split("\n").filter(Boolean);
	} catch {
		return [];
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
	const cwd = data.cwd || process.cwd();
	const project = cwd;
	const toolName = data.tool_name || "unknown";
	const obs_type = OBS_TYPE_MAP[toolName] || "other";
	const toolOutput = data.tool_response || data.tool_output;

	try {
		await fetch(`${REST_URL}/api/v1/agent/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "post_tool_use",
				sessionId,
				project,
				cwd,
				obs_type,
				timestamp: new Date().toISOString(),
				data: {
					tool_name: toolName,
					tool_input: data.tool_input,
					tool_output: truncate(toolOutput, 8000),
				},
			}),
			signal: AbortSignal.timeout(3000),
		});
	} catch {}

	// Detect git commits and send a commit_made observation
	if (toolName === "Bash" || toolName === "Shell") {
		const command = data.tool_input?.command || "";
		if (isGitCommitCommand(command)) {
			let outputStr = "";
			if (typeof toolOutput === "string") {
				try {
					const parsed = JSON.parse(toolOutput);
					outputStr = parsed.stdout || toolOutput;
				} catch {
					outputStr = toolOutput;
				}
			} else if (toolOutput?.stdout) {
				outputStr = toolOutput.stdout;
			} else if (toolOutput != null) {
				outputStr = String(toolOutput);
			}

			const commit = parseCommitOutput(outputStr);
			if (commit) {
				const files = await getCommitFiles(commit.sha, cwd);
				const keywords = commit.message
					.split(/\W+/)
					.filter((w) => w.length > 2);

				try {
					await fetch(`${REST_URL}/api/v1/agent/observe`, {
						method: "POST",
						headers: HEADERS,
						body: JSON.stringify({
							hookType: "post_tool_use",
							sessionId,
							project,
							cwd,
							obs_type: "commit_made",
							timestamp: new Date().toISOString(),
							title: `commit: ${commit.branch}: ${commit.message}`,
							facts: [`sha:${commit.sha}`, `branch:${commit.branch}`],
							keywords,
							files,
							data: {
								tool_name: toolName,
								tool_input: data.tool_input,
							},
							metadata: {
								sha: commit.sha,
								branch: commit.branch,
								message: commit.message,
								files,
							},
							importance: 8,
						}),
						signal: AbortSignal.timeout(3000),
					});
				} catch {}
			}
		}
	}
}

function truncate(value, max) {
	if (typeof value === "string" && value.length > max)
		return value.slice(0, max) + "\n[...truncated]";
	if (typeof value === "object" && value !== null) {
		const str = JSON.stringify(value);
		if (str.length > max) return str.slice(0, max) + "...[truncated]";
		return value;
	}
	return value;
}

main();

//#endregion
export {};
//# sourceMappingURL=post-tool-use.mjs.map
