#!/usr/bin/env node
// Parse a Claude Code JSONL transcript into generic agent-usage records.
//
// Output shape matches hifz's `/api/v1/agent/usage/batch` body element. All
// Claude-/Anthropic-specific knowledge (the JSONL format, cache_*_input_tokens
// field names, the synthetic/local-command skip rules) lives in this file
// and nowhere else — hifz core never sees any of it.

import { createReadStream } from "node:fs";
import { createInterface } from "node:readline";

// Anthropic emits one JSONL row per content block (thinking/text/tool_use)
// of a single API response, all sharing the same message.id. ~40% of
// assistant rows are duplicates by id. Dedup on the parser side so the
// records we produce are 1:1 with billed inferences and don't depend on
// the DB UNIQUE index for correctness.
const AUX_ENTRY_TYPES = new Set(["ai-title", "summary"]);

/**
 * @param {string} path  Path to a Claude Code JSONL transcript.
 * @param {object} [opts]
 * @param {string} [opts.sessionId]  Override session id (else inferred from filename).
 * @param {string} [opts.project]    Override project string.
 * @param {string} [opts.cwd]        cwd of the session (used as fallback project).
 * @returns {Promise<{records: object[], untrackedAuxCount: number}>}
 */
// hifz keys Claude Code sessions by the FIRST dash-segment of the UUID, not
// the full UUID. This isn't a hifz design choice — `session::start` runs
// `CREATE type::record("session:<uuid>")` and SurrealDB's record-id parser
// stops the unquoted id at the first `-`, so `5a8bca58-f11a-...` is stored
// as `session:5a8bca58`. Observations/runs all link by that short key.
// The token records MUST use the same key or they won't join the session
// row (the /sessions/[id] panel and project rollup would show nothing).
// Keep this Claude-ism in the adapter — hifz core stays vendor-neutral.
export function normalizeSessionId(id) {
  const s = String(id ?? "").replace(/^session:/, "");
  const seg = s.split("-")[0];
  return seg || s;
}

export async function parseTranscript(path, opts = {}) {
  const lines = await readJsonl(path);

  const inferredSession = normalizeSessionId(
    opts.sessionId || path.split("/").pop().replace(/\.jsonl$/, ""),
  );

  // Pass 1: index entries by uuid; collect eligible-prompt entries; count
  // auxiliary calls (ai-title, summary) Anthropic billed but didn't put a
  // usage block on.
  const byUuid = new Map();
  const promptByUuid = new Map(); // uuid -> { text, ts }
  const eligiblePromptsByTs = []; // [{uuid, text, ts}], chronological
  let untrackedAuxCount = 0;

  for (const entry of lines) {
    if (!entry || typeof entry !== "object") continue;
    if (entry.type && AUX_ENTRY_TYPES.has(entry.type)) {
      untrackedAuxCount += 1;
      continue;
    }
    const uuid = entry.uuid;
    if (uuid) byUuid.set(uuid, entry);

    if (entry.type !== "user" || entry.message?.role !== "user") continue;
    if (entry.isMeta || entry.isCompactSummary === true) continue;

    const text = userPromptText(entry.message.content);
    if (text == null) continue; // tool-result-only / empty / slash-command

    const promptInfo = { text, ts: entry.timestamp ?? null };
    if (uuid) promptByUuid.set(uuid, promptInfo);
    eligiblePromptsByTs.push({ uuid, ...promptInfo });
  }

  eligiblePromptsByTs.sort((a, b) => (a.ts ?? "").localeCompare(b.ts ?? ""));

  // Pass 2: emit one record per assistant API call, dedup'd on message.id,
  // with prompt resolved by walking parentUuid back to the nearest eligible
  // user message.
  const records = [];
  const seenMsgIds = new Set();
  let turnIndex = 0;

  for (const entry of lines) {
    if (entry?.type !== "assistant" || !entry?.message?.usage) continue;

    const model = entry.message.model ?? "unknown";
    if (model === "<synthetic>") continue;

    const msgId = entry.message?.id;
    if (msgId) {
      if (seenMsgIds.has(msgId)) continue;
      seenMsgIds.add(msgId);
    }

    const usage = entry.message.usage;
    const inputTokens = usage.input_tokens ?? 0;
    const outputTokens = usage.output_tokens ?? 0;
    const cacheCreation = usage.cache_creation_input_tokens ?? 0;
    const cacheRead = usage.cache_read_input_tokens ?? 0;

    const tools = [];
    if (Array.isArray(entry.message.content)) {
      for (const block of entry.message.content) {
        if (block?.type === "tool_use" && block.name) tools.push(block.name);
      }
    }

    const breakdown = {};
    if (cacheCreation > 0) breakdown.cache_creation = cacheCreation;
    if (cacheRead > 0) breakdown.cache_read = cacheRead;

    const total = inputTokens + outputTokens + cacheCreation + cacheRead;
    const externalId =
      msgId ?? entry.uuid ?? `${inferredSession}:${turnIndex}`;

    const promptInfo = resolvePrompt(
      entry,
      byUuid,
      promptByUuid,
      eligiblePromptsByTs,
    );

    records.push({
      // Subagent JSONL files carry the parent session id on each row;
      // fall back to the caller-supplied / filename-inferred id when
      // entry.sessionId is missing (typical for top-level transcripts).
      // Normalize to hifz's short key so the row joins the session record.
      session_id: normalizeSessionId(entry.sessionId ?? inferredSession),
      project: opts.project ?? opts.cwd ?? entry.cwd ?? "global",
      agent: "claude-code",
      provider: "anthropic",
      model,
      external_id: externalId,
      timestamp: entry.timestamp ?? new Date().toISOString(),
      input_tokens: inputTokens,
      output_tokens: outputTokens,
      total_tokens: total,
      prompt: promptInfo?.text ?? null,
      prompt_at: promptInfo?.ts ?? null,
      tools,
      breakdown: Object.keys(breakdown).length > 0 ? breakdown : null,
      // Stamp the per-file aux count on the first emitted record only;
      // re-ingest is idempotent because the same first message.id maps
      // to the same row via the (agent, external_id) UNIQUE index.
      aux_calls: records.length === 0 ? untrackedAuxCount : null,
    });

    turnIndex += 1;
  }

  return { records, untrackedAuxCount };
}

// Returns the cleaned prompt text for an eligible user message, or null
// when the message is not a real prompt (slash command, tool-result-only,
// or empty). Mirrors the legacy filtering rules.
function userPromptText(content) {
  if (typeof content === "string") {
    if (
      content.startsWith("<local-command") ||
      content.startsWith("<command-name")
    ) {
      return null;
    }
    const trimmed = content.trim();
    return trimmed || null;
  }
  if (Array.isArray(content)) {
    let hasToolResult = false;
    const texts = [];
    for (const block of content) {
      if (!block) continue;
      if (block.type === "tool_result") {
        hasToolResult = true;
        continue;
      }
      if (block.type === "text" && typeof block.text === "string") {
        texts.push(block.text);
      }
    }
    const joined = texts.join("\n").trim();
    if (!joined) return null;
    // A user message that's purely tool_result with no real text is not a
    // prompt; we already short-circuit those. Mixed messages (text + image
    // + tool_result) keep their text.
    if (hasToolResult && texts.length === 0) return null;
    return joined;
  }
  return null;
}

// Walk parentUuid backward through the entry graph until we hit a uuid that
// has an eligible prompt. Falls back to the most recent eligible prompt by
// timestamp ≤ assistant timestamp when the chain is broken (e.g., after
// /compact rewrites the transcript prefix).
function resolvePrompt(assistantEntry, byUuid, promptByUuid, byTs) {
  const visited = new Set();
  let cursor = assistantEntry.parentUuid;
  while (cursor && !visited.has(cursor)) {
    visited.add(cursor);
    const hit = promptByUuid.get(cursor);
    if (hit) return hit;
    const parent = byUuid.get(cursor);
    cursor = parent?.parentUuid;
  }
  // Chain broken — pick the latest eligible prompt that happened
  // before this assistant turn.
  const ts = assistantEntry.timestamp ?? "";
  let best = null;
  for (const p of byTs) {
    if ((p.ts ?? "") <= ts) best = p;
    else break;
  }
  return best;
}

async function readJsonl(path) {
  const stream = createReadStream(path, { encoding: "utf-8" });
  const rl = createInterface({ input: stream, crlfDelay: Infinity });
  const out = [];
  for await (const line of rl) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      out.push(JSON.parse(trimmed));
    } catch {
      // Skip malformed lines silently — claude-spend does the same.
    }
  }
  return out;
}

// CLI mode: print parsed records as JSON for one file. Useful for
// verification (`node parse-transcript.mjs <path>`).
if (import.meta.url === `file://${process.argv[1]}`) {
  const path = process.argv[2];
  if (!path) {
    console.error("usage: parse-transcript.mjs <transcript.jsonl>");
    process.exit(2);
  }
  const out = await parseTranscript(path);
  process.stdout.write(JSON.stringify(out, null, 2) + "\n");
}
