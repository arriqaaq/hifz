import type { HookPayload } from "./types.js";

/**
 * Returns a HookPayload for the events that should also flow through `/observe`,
 * or null for events that stay in the lossless ledger only.
 *
 * Hifz hookType vocabulary recognized by src/models.rs `From<&str> for HifzEvent`:
 *   UserPromptSubmit / prompt_submit       — opens a Hifz run
 *   PreToolUse / pre_tool_use              — pre-tool, no run mutation
 *   PostToolUse / post_tool_use            — appends to open run
 *   PostToolUseFailure / post_tool_failure — appends with error
 *   PreCompact / PostCompact
 *   Stop / TaskCompleted                   — closes the run
 *   SessionStart / SessionEnd
 * Anything else falls into Unknown (still recorded as an observation but with no run effect).
 */
export function promote(
  eventType: string,
  evt: any,
  ctx: { cwd: string },
  meta: { sessionId: string },
): HookPayload | null {
  const base = {
    sessionId: meta.sessionId,
    project: ctx.cwd,
    cwd: ctx.cwd,
    timestamp: new Date().toISOString(),
    source: "pi_extension",
  };

  switch (eventType) {
    case "turn_start":
      return {
        ...base,
        hookType: "UserPromptSubmit",
        obs_type: "user_prompt",
        data: { turnIndex: evt?.turnIndex, evt },
      };

    case "input": {
      const text = typeof evt?.text === "string" ? evt.text : "";
      if (!text) return null;
      return {
        ...base,
        hookType: "UserPromptSubmit",
        obs_type: "user_prompt",
        title: text.slice(0, 80),
        keywords: extractKeywords(text),
        data: { prompt: text, evt },
      };
    }

    case "tool_execution_end": {
      const isError = !!evt?.isError;
      const toolName = String(evt?.toolName ?? "");
      const args = evt?.args ?? {};
      const result = evt?.result;
      const detail = extractToolDetail(toolName, args, result);
      return {
        ...base,
        hookType: isError ? "PostToolUseFailure" : "PostToolUse",
        obs_type: isError ? "error" : toolToObsType(toolName),
        title: detail.title,
        files: detail.files,
        keywords: detail.keywords,
        data: {
          tool_name: toolName,
          tool_input: args,
          tool_output: truncateForObserve(result),
          is_error: isError,
          toolCallId: evt?.toolCallId,
          ...detail.extras,
        },
      };
    }

    case "tool_result": {
      // Only promote when the extension result actually carries content/details/error info.
      if (!evt || (evt.content === undefined && evt.details === undefined && evt.isError === undefined)) {
        return null;
      }
      const isError = !!evt.isError;
      return {
        ...base,
        hookType: isError ? "PostToolUseFailure" : "PostToolUse",
        obs_type: isError ? "error" : "tool_result",
        data: { evt: truncateForObserve(evt) },
      };
    }

    case "message_end": {
      // Promote terminal assistant messages.
      const role = evt?.message?.role;
      if (role !== "assistant") return null;
      const m = evt.message ?? {};
      const text = extractAssistantText(m);
      const tools = extractAssistantToolCalls(m);
      return {
        ...base,
        hookType: "task_completed",
        obs_type: "assistant_message",
        title: (text || "(assistant)").slice(0, 80),
        keywords: extractKeywords(text),
        data: {
          content_preview: text.slice(0, 400),
          tool_calls: tools.map((t) => t.name),
          stop_reason: m?.stopReason ?? m?.stop_reason,
          usage: m?.usage,
          message: truncateForObserve(m),
        },
      };
    }

    case "before_provider_request": {
      // Captures: which provider/model is about to be called with which messages and tools.
      // RPC mode cannot see this — it's the value-add of running as a Pi extension.
      const provider = evt?.provider ?? evt?.api;
      const model =
        (typeof evt?.model === "string" && evt.model) ||
        evt?.model?.id ||
        evt?.payload?.model;
      const messages: any[] = Array.isArray(evt?.messages)
        ? evt.messages
        : Array.isArray(evt?.payload?.messages)
          ? evt.payload.messages
          : [];
      const tools: any[] = Array.isArray(evt?.tools)
        ? evt.tools
        : Array.isArray(evt?.payload?.tools)
          ? evt.payload.tools
          : [];
      const systemPromptExcerpt = String(
        evt?.systemPrompt ?? evt?.payload?.system ?? evt?.payload?.systemPrompt ?? "",
      ).slice(0, 200);
      return {
        ...base,
        hookType: "PreToolUse",
        obs_type: "provider_call",
        title: `provider call: ${provider ?? "?"}/${model ?? "?"}`,
        data: {
          provider,
          model,
          messages_summary: messages.map((mm: any) => ({
            role: mm?.role,
            len: messageContentLength(mm?.content),
          })),
          system_prompt_excerpt: systemPromptExcerpt,
          tools: tools.map((t: any) => t?.name).filter(Boolean),
        },
      };
    }

    case "compaction_end": {
      if (!evt?.result || evt?.aborted) return null;
      return {
        ...base,
        hookType: "PostCompact",
        obs_type: "compaction_summary",
        data: { result: evt.result, reason: evt?.reason },
      };
    }

    case "auto_retry_end": {
      if (evt?.success !== false) return null;
      return {
        ...base,
        hookType: "PostToolUseFailure",
        obs_type: "error",
        data: { final_error: evt?.finalError, attempt: evt?.attempt },
      };
    }

    case "extension_error":
      return {
        ...base,
        hookType: "PostToolUseFailure",
        obs_type: "error",
        data: { extensionPath: evt?.extensionPath, event: evt?.event, error: evt?.error },
      };

    case "turn_end":
    case "agent_end":
      // Close the open Hifz run.
      return {
        ...base,
        hookType: "Stop",
        data: { evt },
      };

    default:
      return null;
  }
}

/** Pi tool vocabulary verified at pi-mono/.../extensions/types.ts:874-882. */
function toolToObsType(toolName: string): string {
  switch (toolName.toLowerCase()) {
    case "read":
      return "file_read";
    case "write":
      return "file_write";
    case "edit":
      return "file_edit";
    case "bash":
      return "command_run";
    case "grep":
    case "find":
    case "ls":
      return "search";
    default:
      return "tool_use";
  }
}

interface ToolDetail {
  title: string;
  files: string[];
  keywords: string[];
  extras: Record<string, unknown>;
}

/**
 * Tool-specific structured extraction. Keys land in HookPayload.data so
 * compress.rs treats them as facts and embeds them via the synthetic path.
 */
function extractToolDetail(toolName: string, args: any, result: any): ToolDetail {
  const tool = toolName.toLowerCase();
  const files: string[] = [];
  const keywords: string[] = [tool];
  const extras: Record<string, unknown> = {};
  let title = `${toolName} tool`;

  switch (tool) {
    case "bash": {
      const command = String(args?.command ?? "");
      const out = collectStdout(result);
      title = `bash: ${command.slice(0, 60)}`;
      const exitCode = (result as any)?.exitCode ?? (result as any)?.exit_code;
      if (exitCode !== undefined) extras.exit_code = exitCode;
      const tail = lastLines(out, 3);
      if (tail) extras.stdout_tail = tail;
      const stderr = (result as any)?.stderr;
      if (typeof stderr === "string" && stderr.length > 0) {
        extras.stderr_tail = lastLines(stderr, 3);
      }
      keywords.push(...extractKeywords(command));
      break;
    }
    case "read": {
      const path = String(args?.path ?? args?.file_path ?? "");
      if (path) files.push(path);
      title = `read ${basename(path)}`;
      const out = collectStdout(result);
      if (out) extras.line_count = out.split("\n").length;
      break;
    }
    case "write": {
      const path = String(args?.path ?? args?.file_path ?? "");
      if (path) files.push(path);
      const content = String(args?.content ?? "");
      title = `write ${basename(path)}`;
      extras.byte_count = content.length;
      extras.line_count = content === "" ? 0 : content.split("\n").length;
      break;
    }
    case "edit": {
      const path = String(args?.path ?? args?.file_path ?? "");
      if (path) files.push(path);
      const oldStr = String(args?.old_string ?? "");
      const newStr = String(args?.new_string ?? "");
      title = `edit ${basename(path)}`;
      extras.old_count = oldStr.length;
      extras.new_count = newStr.length;
      extras.delta = newStr.length - oldStr.length;
      break;
    }
    case "grep":
    case "find":
    case "ls": {
      const pattern = String(args?.pattern ?? args?.path ?? "");
      title = `${tool} ${pattern.slice(0, 60)}`;
      const matchCount = countResultLines(result);
      if (matchCount !== undefined) extras.match_count = matchCount;
      const path = String(args?.path ?? "");
      if (path) files.push(path);
      break;
    }
    default: {
      // Best-effort: pull file-shaped fields out of args.
      const path = String(args?.path ?? args?.file_path ?? "");
      if (path) files.push(path);
      title = `${toolName}${path ? `: ${basename(path)}` : ""}`;
    }
  }

  return { title, files, keywords, extras };
}

function basename(path: string): string {
  if (!path) return "";
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return i >= 0 ? path.slice(i + 1) : path;
}

function lastLines(text: string, n: number): string {
  if (!text) return "";
  const lines = text.split("\n");
  return lines.slice(-n).join("\n");
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

function countResultLines(result: unknown): number | undefined {
  if (Array.isArray((result as any)?.content)) return (result as any).content.length;
  const out = collectStdout(result);
  if (!out) return undefined;
  return out.split("\n").filter((l) => l.length > 0).length;
}

function extractAssistantText(message: any): string {
  if (!message) return "";
  if (typeof message.content === "string") return message.content;
  if (Array.isArray(message.content)) {
    return message.content
      .map((b: any) => {
        if (typeof b === "string") return b;
        if (b?.type === "text" && typeof b.text === "string") return b.text;
        return "";
      })
      .join(" ")
      .trim();
  }
  return "";
}

function extractAssistantToolCalls(message: any): { name: string }[] {
  if (!message || !Array.isArray(message.content)) return [];
  return message.content
    .filter((b: any) => b?.type === "toolCall" || b?.type === "tool_use")
    .map((b: any) => ({ name: String(b?.name ?? "") }))
    .filter((b: any) => b.name);
}

function messageContentLength(content: unknown): number {
  if (typeof content === "string") return content.length;
  if (Array.isArray(content)) {
    let n = 0;
    for (const b of content) {
      if (typeof b === "string") n += b.length;
      else if (typeof (b as any)?.text === "string") n += (b as any).text.length;
    }
    return n;
  }
  return 0;
}

const KEYWORD_RE = /[A-Za-z][\w-]{2,}/g;
function extractKeywords(text: string): string[] {
  if (!text) return [];
  return [...new Set((text.match(KEYWORD_RE) ?? []).map((w) => w.toLowerCase()))]
    .filter((w) => w.length >= 3 && !STOP_WORDS.has(w))
    .slice(0, 12);
}
const STOP_WORDS = new Set([
  "the", "and", "for", "with", "this", "that", "from", "have", "will", "into",
  "your", "you", "are", "was", "were", "but", "not", "all", "any", "can",
  "use", "new", "one", "two", "out", "now", "just", "tell", "what", "when",
  "how", "why", "where", "which", "should", "would", "could", "also", "does",
]);

const OBSERVE_MAX_BYTES = 8 * 1024;
function truncateForObserve(v: unknown): unknown {
  if (v === null || v === undefined) return v;
  if (typeof v === "string") {
    return v.length > OBSERVE_MAX_BYTES ? v.slice(0, OBSERVE_MAX_BYTES) + "…[truncated]" : v;
  }
  try {
    const json = JSON.stringify(v);
    if (json.length <= OBSERVE_MAX_BYTES) return v;
    return { __truncated: true, sample: json.slice(0, OBSERVE_MAX_BYTES) };
  } catch {
    return v;
  }
}
