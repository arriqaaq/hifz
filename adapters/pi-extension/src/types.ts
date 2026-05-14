/** Wire shape sent to POST /api/v1/agent/observe. Mirrors Hifz `HookPayload`,
 *  plus optional enrichment fields the Claude Code adapter also sends at top
 *  level (title/facts/keywords/files/metadata/importance). Hifz's HookPayload
 *  serde struct currently ignores those, but matching the wire shape keeps both
 *  adapters aligned for a future observe.rs fix that honours them. */
export interface HookPayload {
  hookType: string;
  sessionId: string;
  project: string;
  cwd: string;
  timestamp: string;
  source?: string;
  obs_type?: string;
  /** Causal parent observation id (e.g. `observation:abc`). Set by the
   *  producer when this event correlates to an earlier one. */
  parentObsId?: string | null;
  data: unknown;
  // Optional enrichment (claude-code parity)
  title?: string;
  facts?: string[];
  keywords?: string[];
  files?: string[];
  metadata?: Record<string, unknown>;
  importance?: number;
}

/** Wire shape sent to POST /api/v1/memories. Mirrors Hifz `RememberReq`. */
export interface MemoryRequest {
  title: string;
  content: string;
  category?: string;
  files?: string[];
  keywords?: string[];
  /** Phase 2: optional LLM-set / caller-provided coarse buckets. */
  tags?: string[];
  /** Phase 4: long-form markdown body for Plan/Design/CodeReview/etc. */
  content_long?: string;
  /** Phase 2 lifecycle: this memory closes/resolves the named one. */
  closes_memory_id?: string;
  /** Phase 2 lifecycle: this memory replaces the named one. */
  supersedes_memory_id?: string;
  project?: string;
  sessionId?: string;
}
