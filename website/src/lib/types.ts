export interface HealthResponse {
  status: string;
  version: string;
  sessions: number;
  runs: number;
  observations: number;
  memories: number;
  commits: number;
  uptime_seconds: number;
}

export interface Session {
  id: string;
  project: string;
  cwd: string;
  name: string | null;
  started_at: string;
  ended_at: string | null;
  status: string;
  observation_count: number;
  model: string | null;
  tags: string[] | null;
}

export interface Observation {
  id: string;
  session_id: string | RecordId | null;
  project?: string;
  timestamp: string;
  obs_type: string;
  title: string;
  subtitle: string | null;
  facts: string[];
  narrative: string;
  keywords: string[];
  files: string[];
  importance: number;
  confidence: number | null;
  metadata?: Record<string, unknown> | null;
}

export interface EvolutionEntry {
  timestamp: string;
  field: string;
  previous: string | null;
  reason: string;
  triggered_by: string | null;
}

export interface Memory {
  id: string;
  project: string;
  category: string;
  title: string;
  content: string;
  /** Phase 4: long-form markdown body for Plan/Design/CodeReview/ShipReport/ContextSlice. */
  content_long?: string | null;
  keywords: string[];
  files: string[];
  tags: string[];
  /** Legacy free-form context line (Phase 1 schema). */
  context: string | null;
  /** Phase 2: A-MEM context_summary — LLM-generated paragraph. */
  context_summary?: string | null;
  /** Phase 2: append-only audit log of LLM rewrites. */
  evolution_history?: EvolutionEntry[];
  strength: number;
  retrieval_count: number;
  last_accessed_at: string;
  version: number;
  parent_id: string | null;
  supersedes: string[] | null;
  is_latest: boolean;
  pinned: boolean;
  forget_after: string | null;
  created_at: string;
  updated_at: string;
}

export interface MemoryEdge {
  id?: string;
  title?: string;
  category?: string;
  context_summary?: string | null;
  relation: string;
  via: string;
  score: number;
  reason?: string | null;
}

export interface NeighborsResponse {
  neighbors: MemoryEdge[];
  count: number;
}

export interface BacklinksResponse {
  backlinks: MemoryEdge[];
  count: number;
}

export interface WarmupEntry {
  id: string;
  category: string;
  title: string;
  summary: string;
  strength: number;
  retrieval_count: number;
  last_accessed_at: string;
}

export interface WarmupDigest {
  project: string;
  session_id: string | null;
  latest_plan: WarmupEntry | null;
  decisions: WarmupEntry[];
  conventions: WarmupEntry[];
  open_bugs: WarmupEntry[];
  gotchas: WarmupEntry[];
  failure_patterns: WarmupEntry[];
  recent_lessons: WarmupEntry[];
  top: WarmupEntry[];
}

export interface ProjectDigestByCategory {
  project: string;
  days: number;
  since: string;
  by_category: Record<string, Array<{ id: string; title: string; summary: string; created_at: string }>>;
}

export interface CoreMemory {
  project: string;
  identity: string | null;
  goals: string[];
  invariants: string[];
  watchlist: string[];
  updated_at: string;
}

export interface Run {
  id: string;
  session_id: string | RecordId;
  project: string;
  started_at: string;
  ended_at: string | null;
  prompt: string;
  prompts?: string[];
  outcome: string;
  observation_ids: string[];
  recalled_ids?: string[];
  lesson: string | null;
}

export interface RunDetail {
  run: Run;
  observations: Observation[];
}

export interface RecordId {
  table: string;
  key: { String?: string; Number?: number } | string;
}

// Commits are stored as observations (obs_type='commit_made').
// sha, branch, message, files are in the metadata field.
export interface Commit extends Observation {
  metadata: {
    sha: string;
    branch: string;
    message: string;
    files: string[];
  };
}

export interface KeywordFreq {
  keyword: string;
  frequency: number;
}

export interface FileFreq {
  file: string;
  frequency: number;
}

export interface ProjectDigest {
  project: string;
  updated_at: string;
  top_keywords: KeywordFreq[];
  top_files: FileFreq[];
  session_count: number;
  total_observations: number;
}

export interface SearchResult {
  id: string;
  session_id: string | null;
  title: string;
  obs_type: string;
  narrative: string;
  timestamp: string;
  importance: number;
  score: number | null;
  is_neighbor: boolean;
}

export interface RememberRequest {
  title: string;
  content: string;
  category?: string;
  keywords?: string[];
  files?: string[];
  tags?: string[];
  content_long?: string | null;
  closes_memory_id?: string;
  supersedes_memory_id?: string;
  project?: string;
  session_id?: string;
}

export interface CoreEditRequest {
  project: string;
  field: 'identity' | 'goals' | 'invariants' | 'watchlist';
  op: 'set' | 'add' | 'remove';
  value: string;
}

// --- Renderer (memdiff) wire types: mirror crates/memdiff serde shapes ---
export type Tone =
  | 'plain' | 'added' | 'revised' | 'removed'
  | 'linked' | 'conflict' | 'muted' | 'cite';
export type ChangeOp =
  | 'created' | 'revised' | 'superseded' | 'linked'
  | 'neighbour_revised' | 'forgotten' | 'conflict';
export type Glyph =
  | 'plus' | 'tilde' | 'slashed' | 'arrow' | 'recycle' | 'cross' | 'bang';

export interface SpanStyle {
  tone: Tone;
  bold?: boolean;
  dim?: boolean;
  strike?: boolean;
}
export type Cite =
  | { kind: 'memory'; id: string }
  | { kind: 'edge'; relation: string; target: string }
  | { kind: 'run'; id: string };
export interface Span {
  text: string;
  style: SpanStyle;
  cite?: Cite;
}
export interface DeltaLine {
  op: ChangeOp;
  glyph: Glyph;
  spans: Span[];
}
export interface MemoryDelta {
  lines: DeltaLine[];
}
export interface MemoryView {
  header: Span[];
  rows: DeltaLine[];
}
export type SessionEvent =
  | { kind: 'prompt'; t: string; text: string }
  | { kind: 'delta'; t: string; delta: MemoryDelta }
  | { kind: 'view'; t: string; view: MemoryView }
  | { kind: 'note'; t: string; text: string }
  | { kind: 'error'; t: string; message: string };
export interface ReplaySession {
  session_id: string;
  count: number;
  last_ts: string;
}
export interface ReplayDetail {
  session_id: string;
  events: SessionEvent[];
  count: number;
}
export type RenderTokens = Record<Tone, string>;
