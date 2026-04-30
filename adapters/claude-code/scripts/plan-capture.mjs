#!/usr/bin/env node
//#region src/hooks/plan-capture.ts
import { readFile } from "node:fs/promises";

const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };
const PLAN_PATH_RE = /\.claude\/plans\/.+\.md$/;

async function main() {
	let input = "";
	for await (const chunk of process.stdin) input += chunk;
	let data;
	try {
		data = JSON.parse(input);
	} catch {
		return;
	}
	if (data.tool_name !== "Write") return;

	const filePath = data.tool_input?.file_path;
	if (typeof filePath !== "string" || !PLAN_PATH_RE.test(filePath)) return;

	let content;
	try {
		content = await readFile(filePath, "utf8");
	} catch {
		return;
	}
	if (!content || content.length < 10) return;

	// Extract title from first # heading, fallback to filename
	const titleMatch = content.match(/^#\s+(.+)$/m);
	const title = titleMatch
		? titleMatch[1].trim()
		: filePath.split("/").pop().replace(/\.md$/, "");

	// Extract keywords: ## section headers + key concepts
	const keywords = new Set();
	const fileRefs =
		content.match(/[\w/.-]+\.(rs|mjs|ts|json|md|toml|yaml|yml|py|sh)\b/g) ||
		[];
	for (const ref of fileRefs.slice(0, 10)) keywords.add(ref.toLowerCase());
	const sections = content.match(/^##\s+(.+)$/gm) || [];
	for (const sec of sections.slice(0, 5)) {
		const cleaned = sec
			.replace(/^##\s+/, "")
			.toLowerCase()
			.replace(/[^a-z0-9]+/g, "-")
			.replace(/^-|-$/g, "");
		if (cleaned.length > 2) keywords.add(cleaned);
	}

	// Extract files list
	const files = Array.from(new Set(fileRefs.slice(0, 20)));

	// Truncate content to 8KB
	const MAX = 8000;
	const truncated =
		content.length > MAX ? content.slice(0, MAX) + "\n\n[...truncated]" : content;

	const project = data.cwd || process.cwd();

	try {
		await fetch(`${REST_URL}/api/v1/memories`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				category: "plan",
				title,
				content: truncated,
				project,
				keywords: Array.from(keywords),
				files,
				tags: ["active"],
				pinned: true,
				context: filePath,
			}),
			signal: AbortSignal.timeout(3000),
		});
	} catch {}
}
main();

//#endregion
export {};
