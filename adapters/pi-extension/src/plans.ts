import { readFileSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";
import type { MemoryRequest } from "./types.js";

const DEFAULT_PLAN_GLOBS = [
  /\.claude\/plans\/.+\.md$/,
  /\.pi\/plans\/.+\.md$/,
  /\.cursor\/plans\/.+\.md$/,
];

const TITLE_RE = /^#\s+(.+?)\s*$/m;
const SECTION_RE = /^##\s+(.+?)\s*$/gm;
const FILE_REF_RE = /(?<![\w/.-])([\w][\w./-]*\.(?:rs|mjs|ts|tsx|js|jsx|json|md|toml|yaml|yml|py|sh|sql))/g;
const KEYWORD_RE = /[A-Za-z][\w-]{2,}/g;

/**
 * Capture plan-file writes/edits as Hifz `category=plan` memories.
 *
 * Triggers on `tool_execution_end` for `write` (full content in args) or
 * `edit` (diff in args; full post-edit content read from disk).
 *
 * Mirrors adapters/claude-code/scripts/plan-capture.mjs.
 */
export function detectPlan(
  evt: any,
  ctx: { cwd: string },
  meta: { sessionId: string },
  customGlob?: RegExp,
): MemoryRequest | null {
  if (!evt) return null;
  const path = String(evt?.args?.path ?? evt?.args?.file_path ?? "");
  if (!path) return null;

  const globs = customGlob ? [customGlob, ...DEFAULT_PLAN_GLOBS] : DEFAULT_PLAN_GLOBS;
  if (!globs.some((re) => re.test(path))) return null;

  const toolName = String(evt?.toolName ?? "").toLowerCase();
  let content = "";

  if (toolName === "write" && typeof evt?.args?.content === "string") {
    content = evt.args.content;
  } else {
    // For edit, args carry only old_string/new_string. Read post-edit file from disk.
    // Also a fallback path for write-tool result-only shapes.
    const abs = isAbsolute(path) ? path : resolve(ctx.cwd, path);
    try {
      content = readFileSync(abs, "utf8");
    } catch {
      // File unreadable — try the result string as a last resort.
      content = typeof evt?.result === "string" ? evt.result : "";
    }
  }

  if (!content) return null;

  // Phase 9: route plan files through `content_long` so Phase 4's chunker
  // splits them for retrieval. The `content` field gets a short summary
  // (first ~300 chars) so embedded BM25/vec search still works against it.
  const fullBody = content; // keep entire plan body as content_long
  const summary =
    fullBody.length > 300 ? fullBody.slice(0, 300) + "…" : fullBody;

  const title = (fullBody.match(TITLE_RE)?.[1] ?? path.split("/").pop() ?? "plan").trim();
  const sections = [...fullBody.matchAll(SECTION_RE)]
    .map((m) => (m[1] ?? "").trim())
    .filter((s) => s.length > 0)
    .slice(0, 16);
  const files = [...new Set(
    [...fullBody.matchAll(FILE_REF_RE)]
      .map((m) => m[1])
      .filter((f): f is string => typeof f === "string"),
  )].slice(0, 32);
  const keywords = (fullBody.match(KEYWORD_RE) ?? [])
    .filter((w) => w.length >= 4)
    .slice(0, 32);

  return {
    title,
    content: summary,
    content_long: fullBody,
    category: "plan",
    files,
    keywords: [...keywords, ...sections.slice(0, 4)],
    project: ctx.cwd,
    sessionId: meta.sessionId,
  };
}
