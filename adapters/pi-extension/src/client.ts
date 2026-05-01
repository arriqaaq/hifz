import { Spool } from "./spool.js";
import type { EventRequest, HookPayload, MemoryRequest } from "./types.js";

interface ClientOpts {
  url: string;
  spoolDir: string;
  source: string;
}

const FETCH_TIMEOUT_MS = 5000;

type SpoolKind = "event" | "observation" | "memory" | "consolidate" | "session";

export class Client {
  private opts: ClientOpts;
  private spool: Spool;
  /** True after the first failed POST; cleared once spool drains successfully. */
  private degraded = false;

  constructor(opts: ClientOpts) {
    this.opts = opts;
    this.spool = new Spool(opts.spoolDir);
  }

  async startSession(body: { sessionId: string; project: string; cwd: string }): Promise<{ sessionId: string }> {
    const r = await this.post<{ sessionId?: string }>(
      "/api/v1/agent/sessions",
      body,
      "session",
    );
    return { sessionId: r?.sessionId ?? body.sessionId };
  }

  async endSession(body: { sessionId: string }): Promise<void> {
    await this.post("/api/v1/agent/sessions/end", body, "session");
  }

  async sendEvent(ev: EventRequest): Promise<void> {
    await this.post("/api/v1/agent/events", ev, "event");
  }

  /**
   * Promote an event to a Hifz observation. Hifz does not currently surface the run id
   * via /observe, so callers cannot stamp event.run_id from the response. Run linkage
   * is preserved at the observation level via run.observation_ids (see observe.rs).
   */
  async sendObservation(payload: HookPayload): Promise<void> {
    await this.post("/api/v1/agent/observe", payload, "observation");
  }

  async sendMemory(mem: MemoryRequest): Promise<void> {
    await this.post("/api/v1/memories", mem, "memory");
  }

  async consolidate(): Promise<void> {
    // /api/v1/consolidate takes no body; send {} to be explicit.
    await this.post("/api/v1/consolidate", {}, "consolidate");
  }

  /** Read-only: search Hifz observations / memories. Used by failure-recovery and /hifz-recall. */
  async searchAgentic(req: { query: string; limit?: number; project?: string; sessionId?: string }): Promise<{ results: any[] } | null> {
    const r = await this.request("/api/v1/search/agentic", req);
    if (!r.ok) return null;
    const j = r.json as { results?: any[] } | null;
    return { results: j?.results ?? [] };
  }

  /** Read-only: fetch Hifz's synthesised context for a project. Used by compaction snapshot. */
  async fetchContext(req: { project: string; tokenBudget?: number; query?: string }): Promise<string | null> {
    const r = await this.request("/api/v1/context", {
      project: req.project,
      token_budget: req.tokenBudget ?? 1500,
      query: req.query,
    });
    if (!r.ok) return null;
    if (typeof r.json === "string") return r.json;
    if (r.json && typeof (r.json as any).context === "string") return (r.json as any).context;
    return null;
  }

  /** Drain spool: best-effort replay; on first failure stop and remain degraded. */
  async drainSpool(): Promise<void> {
    const drained = new Set<string>();
    for (const item of this.spool.drain()) {
      let parsed: { kind: SpoolKind; body: unknown };
      try {
        parsed = JSON.parse(item.line);
      } catch {
        continue;
      }
      const path = endpointFor(parsed.kind);
      if (!path) continue;
      const res = await this.request(path, parsed.body);
      if (!res.ok) {
        this.degraded = true;
        return;
      }
      drained.add(item.file);
    }
    for (const f of drained) this.spool.remove(f);
    this.degraded = false;
  }

  // --- internals ---

  /** One round-trip: returns parsed JSON body on success, null on any failure. Spools on failure. */
  private async post<T = unknown>(path: string, body: unknown, kind: SpoolKind): Promise<T | null> {
    if (this.degraded) {
      this.spool.append(kind, body);
      return null;
    }
    const res = await this.request(path, body);
    if (!res.ok) {
      this.degraded = true;
      this.spool.append(kind, body);
      return null;
    }
    return res.json as T | null;
  }

  /** Single fetch + parse. Returns ok flag and parsed body (or null on parse failure). */
  private async request(path: string, body: unknown): Promise<{ ok: boolean; json: unknown | null }> {
    if (!this.opts.url) return { ok: false, json: null };
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), FETCH_TIMEOUT_MS);
    try {
      const res = await fetch(this.opts.url + path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        signal: ctrl.signal,
      });
      clearTimeout(t);
      if (!res.ok) return { ok: false, json: null };
      let json: unknown = null;
      try {
        json = await res.json();
      } catch {
        json = null;
      }
      return { ok: true, json };
    } catch {
      clearTimeout(t);
      return { ok: false, json: null };
    }
  }
}

function endpointFor(kind: SpoolKind): string | null {
  switch (kind) {
    case "event":
      return "/api/v1/agent/events";
    case "observation":
      return "/api/v1/agent/observe";
    case "memory":
      return "/api/v1/memories";
    case "consolidate":
      return "/api/v1/consolidate";
    case "session":
      return "/api/v1/agent/sessions";
    default:
      return null;
  }
}
