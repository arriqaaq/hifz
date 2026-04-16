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
	const title = titleMatch ? titleMatch[1].trim() : filePath.split("/").pop().replace(/\.md$/, "");

	// Extract concepts: file paths referenced + ## section headers
	const concepts = new Set();
	const fileRefs = content.match(/[\w/.-]+\.(rs|mjs|ts|json|md|toml|yaml|yml|py|sh)\b/g) || [];
	for (const ref of fileRefs.slice(0, 10)) concepts.add(ref.toLowerCase());
	const sections = content.match(/^##\s+(.+)$/gm) || [];
	for (const sec of sections.slice(0, 5)) {
		const cleaned = sec.replace(/^##\s+/, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
		if (cleaned.length > 2) concepts.add(cleaned);
	}

	// Extract files list: same file refs used for concepts
	const files = Array.from(new Set(fileRefs.slice(0, 20)));

	// Truncate content to 8KB
	const MAX = 8000;
	const truncated = content.length > MAX ? content.slice(0, MAX) + "\n\n[...truncated]" : content;

	try {
		await fetch(`${REST_URL}/hifz/remember`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				title: `Plan: ${title}`,
				content: truncated,
				type: "architecture",
				concepts: Array.from(concepts),
				files
			}),
			signal: AbortSignal.timeout(3000)
		});
	} catch {}
}
main();

//#endregion
export {};
