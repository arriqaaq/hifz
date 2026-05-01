import { execSync } from "node:child_process";
import type { HookPayload } from "./types.js";

const COMMIT_RE = /\[([^\s\]]+)\s+([a-f0-9]{7,40})\]\s*(.*)/;
const KEYWORD_RE = /[A-Za-z][\w-]{2,}/g;

/**
 * If a `bash` tool_execution_end ran `git commit`, parse stdout for the SHA / branch / message
 * and run `git diff-tree` locally to enumerate touched files. Returns a `commit_made` HookPayload
 * suitable for POST /observe, or null if the command isn't a commit (or didn't succeed).
 *
 * Mirrors adapters/claude-code/scripts/post-tool-use.mjs git enrichment.
 */
export function detectCommit(
  evt: any,
  ctx: { cwd: string },
  meta: { sessionId: string },
): HookPayload | null {
  if (!evt) return null;
  const command = String(evt?.args?.command ?? "");
  if (!/\bgit\s+commit\b/.test(command)) return null;

  const output = collectStdout(evt?.result);
  const m = output.match(COMMIT_RE);
  if (!m) return null;
  const branch = m[1];
  const sha = m[2];
  const subject = m[3]?.trim() ?? "";

  // Enumerate files via `git diff-tree`. Failures are non-fatal.
  let files: string[] = [];
  try {
    const out = execSync(`git -C ${shellQuote(ctx.cwd)} diff-tree --no-commit-id --name-only -r ${sha}`, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 3000,
    });
    files = out.split("\n").filter((line) => line.length > 0);
  } catch {
    files = [];
  }

  const keywords = (subject.match(KEYWORD_RE) ?? [])
    .filter((w) => w.length >= 3)
    .slice(0, 16);

  return {
    sessionId: meta.sessionId,
    project: ctx.cwd,
    cwd: ctx.cwd,
    timestamp: new Date().toISOString(),
    source: "pi_extension",
    hookType: "PostToolUse",
    obs_type: "commit_made",
    // Top-level enrichment matches claude-code/scripts/post-tool-use.mjs commit_made shape.
    title: `commit: ${branch}: ${subject || sha}`,
    facts: [`sha:${sha}`, `branch:${branch}`],
    keywords,
    files,
    metadata: { sha, branch, message: subject, files },
    importance: 8,
    // `data` carries the raw tool I/O so compress.rs has the bash command
    // and stdout to synthesize a sensible title/narrative even when top-level
    // enrichment is dropped by serde.
    data: {
      tool_name: "bash",
      tool_input: { command },
      tool_output: output,
    },
  };
}

function collectStdout(result: unknown): string {
  if (typeof result === "string") return result;
  if (result && typeof result === "object") {
    const r = result as Record<string, unknown>;
    if (typeof r.stdout === "string") return r.stdout;
    if (typeof r.output === "string") return r.output;
    if (Array.isArray(r.content)) {
      return r.content
        .map((c: any) => (typeof c === "string" ? c : typeof c?.text === "string" ? c.text : ""))
        .join("\n");
    }
  }
  return "";
}

function shellQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}
