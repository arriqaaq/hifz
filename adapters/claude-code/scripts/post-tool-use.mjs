#!/usr/bin/env node
//#region src/hooks/post-tool-use.ts
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
	// Debug: log keys and tool_output presence
	const fs = await import("fs");
	const debugLine = `${new Date().toISOString()} keys=${Object.keys(data).join(",")} tool_name=${data.tool_name} has_tool_output=${data.tool_output != null} has_output=${data.output != null} has_result=${data.result != null} tool_output_type=${typeof data.tool_output} output_type=${typeof data.output}\n`;
	fs.appendFileSync("/tmp/hifz-hook-debug.log", debugLine);
	try {
		await fetch(`${REST_URL}/api/v1/agent/observe`, {
			method: "POST",
			headers: HEADERS,
			body: JSON.stringify({
				hookType: "post_tool_use",
				sessionId,
				project: data.cwd || process.cwd(),
				cwd: data.cwd || process.cwd(),
				timestamp: (/* @__PURE__ */ new Date()).toISOString(),
				data: {
					tool_name: data.tool_name,
					tool_input: data.tool_input,
					tool_output: truncate(data.tool_response || data.tool_output, 8e3)
				}
			}),
			signal: AbortSignal.timeout(3e3)
		});
	} catch {}
}
function truncate(value, max) {
	if (typeof value === "string" && value.length > max) return value.slice(0, max) + "\n[...truncated]";
	if (typeof value === "object" && value !== null) {
		const str = JSON.stringify(value);
		if (str.length > max) return str.slice(0, max) + "...[truncated]";
		return value;
	}
	return value;
}
main();

//#endregion
export {  };
//# sourceMappingURL=post-tool-use.mjs.map