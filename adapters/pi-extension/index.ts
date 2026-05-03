// Hifz logger — Pi extension that captures every Pi event into the local Hifz store.
//
// Loads via Pi's extension discovery (~/.pi/extensions/<name>/index.ts).
// Subscribes to every meaningful Pi hook, dual-writes:
//   - every event → POST /api/v1/agent/events  (lossless ledger, no embedding)
//   - selected events → POST /api/v1/agent/observe / /memories / /consolidate
// See plan: /Users/kfarhan/.claude/plans/can-you-create-a-gleaming-manatee.md

import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent";
import { Client } from "./src/client.js";
import { detectCommit } from "./src/git.js";
import { hashEvent } from "./src/hash.js";
import { CATEGORIES } from "./src/ontology.js";
import { detectPlan } from "./src/plans.js";
import { promote } from "./src/promote.js";
import { redact } from "./src/redact.js";

const RPC_VISIBLE_HOOKS = [
  "agent_start",
  "agent_end",
  "turn_start",
  "turn_end",
  "message_start",
  "message_end",
  "tool_execution_start",
  "tool_execution_end",
  "queue_update",
  "compaction_start",
  "compaction_end",
  "auto_retry_start",
  "auto_retry_end",
  "session_info_changed",
  "model_select",
] as const;

const EXTENSION_ONLY_HOOKS = [
  "input",
  "context",
  "before_agent_start",
  "before_provider_request",
  "after_provider_response",
  "tool_call",
  "tool_result",
  "user_bash",
  "session_start",
  "session_before_switch",
  "session_before_fork",
  "session_before_compact",
  "session_compact",
  "session_shutdown",
  "session_before_tree",
  "session_tree",
  "resources_discover",
] as const;

const HIGH_VOLUME_HOOKS = ["message_update", "tool_execution_update"] as const;

/** Hooks where we await the handler so the final round-trips finish before Pi exits. */
const TERMINAL_HOOKS = new Set(["agent_end", "session_shutdown"]);

export default async function (pi: ExtensionAPI): Promise<void> {
  const url = process.env.HIFZ_URL ?? "http://localhost:3111";
  if (url.length === 0) return;

  const spoolDir =
    process.env.HIFZ_SPOOL ?? `${process.env.HOME ?? ""}/.hifz/spool/pi-extension`;
  const planGlob = process.env.HIFZ_PLAN_GLOB ? new RegExp(process.env.HIFZ_PLAN_GLOB) : undefined;

  const client = new Client({ url, spoolDir, source: "pi_extension" });
  void client.drainSpool();

  let sessionId: string | undefined;
  let sequence = 0;

  async function ensureSession(ctx: ExtensionContext): Promise<string> {
    if (sessionId) return sessionId;
    const sm = ctx.sessionManager as unknown as { getSessionId?: () => string };
    const piSessionId = typeof sm?.getSessionId === "function" ? sm.getSessionId() : "";
    const candidate =
      piSessionId && piSessionId.length > 0
        ? piSessionId
        : typeof crypto !== "undefined" && "randomUUID" in crypto
          ? (crypto as { randomUUID: () => string }).randomUUID()
          : `pi-${Date.now()}`;
    const res = await client.startSession({
      sessionId: candidate,
      project: ctx.cwd,
      cwd: ctx.cwd,
    });
    sessionId = res.sessionId;
    return sessionId;
  }

  async function handle(eventType: string, evt: unknown, ctx: ExtensionContext): Promise<void> {
    try {
      const sid = await ensureSession(ctx);
      const seq = ++sequence;

      // 1) Always: lossless ledger.
      const redactedPayload = redact(evt);
      const eventBody = {
        source: "pi_extension",
        event_type: eventType,
        session_id: sid,
        run_id: null,
        sequence: seq,
        timestamp: new Date().toISOString(),
        parent_event_id: null,
        payload_hash: hashEvent(sid, eventType, seq, evt),
        payload:
          redactedPayload && typeof redactedPayload === "object"
            ? (redactedPayload as object)
            : { value: redactedPayload },
      };
      await client.sendEvent(eventBody);

      // 2) Selective promotion → /observe.
      const obs = promote(eventType, evt as any, ctx, { sessionId: sid });
      if (obs) {
        // 2a) Failure recovery: attach similar past failures to error observations.
        if (obs.obs_type === "error") {
          const e = evt as any;
          const tool = String(e?.toolName ?? "");
          const errSnippet = errorSnippet(e);
          const q = `${tool} ${errSnippet}`.trim().slice(0, 240);
          if (q) {
            const r = await client.searchAgentic({ query: q, limit: 5, project: ctx.cwd });
            const similar = (r?.results ?? [])
              .map((x: any) => ({
                title: x?.title,
                obs_type: x?.obs_type,
                score: x?.score,
                session_id: x?.session_id,
              }))
              .slice(0, 3);
            obs.data = { ...(obs.data as object), similar_past_failures: similar };
          }
        }
        await client.sendObservation(obs);
      }

      // 3) Enrichment: git commit detection on bash tool ends.
      if (eventType === "tool_execution_end" && (evt as any)?.toolName === "bash") {
        const commitObs = detectCommit(evt, ctx, { sessionId: sid });
        if (commitObs) await client.sendObservation(commitObs);
      }

      // 4) Enrichment: plan capture on write OR edit tool ends.
      if (
        eventType === "tool_execution_end" &&
        ((evt as any)?.toolName === "write" || (evt as any)?.toolName === "edit")
      ) {
        const plan = detectPlan(evt, ctx, { sessionId: sid }, planGlob);
        if (plan) await client.sendMemory(plan);
      }

      // 5) Enrichment: pre-compaction context snapshot.
      if (eventType === "session_before_compact") {
        const ctxText = await client.fetchContext({ project: ctx.cwd, tokenBudget: 1500 });
        if (ctxText) {
          await client.sendObservation({
            sessionId: sid,
            project: ctx.cwd,
            cwd: ctx.cwd,
            timestamp: new Date().toISOString(),
            source: "pi_extension",
            hookType: "PreCompact",
            obs_type: "compaction_context",
            title: "compaction context snapshot",
            data: {
              context_excerpt: ctxText.slice(0, 8 * 1024),
              reason: (evt as any)?.reason,
            },
          });
        }
      }

      // 6) Session-end consolidation.
      if (eventType === "session_shutdown" || eventType === "agent_end") {
        await client.consolidate();
        if (eventType === "session_shutdown") {
          await client.endSession({ sessionId: sid });
        }
      }
    } catch (err) {
      // Never throw out of a Pi hook.
      // eslint-disable-next-line no-console
      console.error("[hifz-logger] handler error:", err);
    }
  }

  const wireHook = (t: string) => {
    pi.on(t as any, async (e: unknown, c: ExtensionContext) => {
      if (TERMINAL_HOOKS.has(t)) await handle(t, e, c);
      else void handle(t, e, c);
    });
  };

  for (const t of RPC_VISIBLE_HOOKS) wireHook(t);
  for (const t of EXTENSION_ONLY_HOOKS) wireHook(t);
  if (process.env.HIFZ_VERBOSE_DELTAS === "1") {
    for (const t of HIGH_VOLUME_HOOKS) wireHook(t);
  }

  // ----- User-facing slash commands -----

  pi.registerCommand("hifz-remember", {
    description:
      "Save the rest of the line as a Hifz memory. Format: [--category=<typed>] <title> :: <content>",
    handler: async (args: string, ctx: ExtensionContext) => {
      // Parse optional `--category=foo` / `--cat=foo` prefix.
      let rest = (args ?? "").trim();
      let category = "note"; // Phase 9: typed default (was "insight" — not in Category enum)
      const catMatch = rest.match(/^--(?:category|cat)=([a-z_]+)\s+/);
      if (catMatch?.[1]) {
        category = catMatch[1];
        rest = rest.slice(catMatch[0].length);
      }
      if (!(CATEGORIES as readonly string[]).includes(category)) {
        if (ctx.hasUI)
          ctx.ui.notify(
            `Unknown category "${category}". Valid: ${CATEGORIES.join(", ")}`,
            "error",
          );
        return;
      }
      const [titleRaw, ...contentParts] = rest.split("::");
      const title = (titleRaw ?? "").trim();
      const content = contentParts.join("::").trim();
      if (!title || !content) {
        if (ctx.hasUI)
          ctx.ui.notify(
            "Usage: /hifz-remember [--category=<lesson|decision|bug|fix|note|...>] <title> :: <content>",
            "error",
          );
        return;
      }
      try {
        await client.sendMemory({ title, content, category, project: ctx.cwd });
        if (ctx.hasUI) ctx.ui.notify(`Saved ${category}: ${title}`, "info");
      } catch (err) {
        if (ctx.hasUI) ctx.ui.notify(`hifz-remember failed: ${err}`, "error");
      }
    },
  });

  pi.registerCommand("hifz-recall", {
    description: "Search Hifz observations and memories.",
    handler: async (args: string, ctx: ExtensionContext) => {
      const query = (args ?? "").trim();
      if (!query) {
        if (ctx.hasUI) ctx.ui.notify("Usage: /hifz-recall <query>", "error");
        return;
      }
      try {
        const r = await client.searchAgentic({ query, limit: 5, project: ctx.cwd });
        const lines = (r?.results ?? [])
          .map((x: any) => `• ${x?.title ?? "(untitled)"} [${x?.obs_type ?? "?"}]`)
          .join("\n");
        if (ctx.hasUI) ctx.ui.notify(lines || "no matches", "info");
      } catch (err) {
        if (ctx.hasUI) ctx.ui.notify(`hifz-recall failed: ${err}`, "error");
      }
    },
  });
}

function errorSnippet(evt: any): string {
  const r = evt?.result;
  if (typeof r === "string") return r.slice(0, 200);
  if (r && typeof r === "object") {
    const arr = (r as any).content;
    if (Array.isArray(arr)) {
      for (const b of arr) {
        if (typeof b?.text === "string") return b.text.slice(0, 200);
      }
    }
    if (typeof (r as any).error === "string") return (r as any).error.slice(0, 200);
    if (typeof (r as any).message === "string") return (r as any).message.slice(0, 200);
  }
  return "";
}
