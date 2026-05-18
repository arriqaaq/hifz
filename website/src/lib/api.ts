import type {
  HealthResponse,
  Session,
  Observation,
  SearchResult,
  CoreMemory,
  CoreEditRequest,
  Run,
  RunDetail,
  ProjectDigest,
  Commit,
  RememberRequest,
  Memory,
  NeighborsResponse,
  BacklinksResponse,
  WarmupDigest,
  ProjectDigestByCategory,
  MemoryView,
  ReplaySession,
  ReplayDetail,
  RenderTokens,
} from './types';

const CORE = '/api/v1';
const AGENT = '/api/v1/agent';

async function get<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`GET ${url}: ${res.status}`);
  return res.json();
}

async function post<T>(url: string, body?: unknown): Promise<T> {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) throw new Error(`POST ${url}: ${res.status}`);
  return res.json();
}

async function patch<T>(url: string, body?: unknown): Promise<T> {
  const res = await fetch(url, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) throw new Error(`PATCH ${url}: ${res.status}`);
  return res.json();
}

async function del<T>(url: string, body?: unknown): Promise<T> {
  const res = await fetch(url, {
    method: 'DELETE',
    headers: { 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) throw new Error(`DELETE ${url}: ${res.status}`);
  return res.json();
}

// --- Core Memory API ---

export function getHealth(): Promise<HealthResponse> {
  return get(`${CORE}/health`);
}

export function smartSearch(
  query: string,
  limit = 10,
  mode = 'hybrid',
  project?: string,
): Promise<{ results: SearchResult[]; count: number }> {
  return post(`${CORE}/search`, { query, limit, mode, project });
}

export function searchAgentic(
  query: string,
  limit = 10,
  project?: string,
): Promise<{ results: SearchResult[]; count: number }> {
  return post(`${CORE}/search/agentic`, { query, limit, project });
}

export function remember(body: RememberRequest): Promise<{ status: string; title: string }> {
  return post(`${CORE}/memories`, body);
}

export function forget(id: string): Promise<{ status: string }> {
  return del(`${CORE}/memories`, { id });
}

export function searchMemories(
  query?: string,
  limit = 50,
  project?: string,
  category?: string,
  opts?: { since?: string; open?: boolean },
): Promise<{ memories: Memory[]; count: number }> {
  const params = new URLSearchParams();
  if (query) params.set('query', query);
  if (project) params.set('project', project);
  if (category) params.set('category', category);
  if (opts?.since) params.set('since', opts.since);
  if (opts?.open) params.set('open', 'true');
  params.set('limit', String(limit));
  return get(`${CORE}/memories?${params}`);
}

// --- Phase 4-6 endpoints (typed graph + markdown round-trip + warmup) ---

export function getMemoryNeighbors(
  id: string,
  opts: { relations?: string[]; maxHops?: number } = {},
): Promise<NeighborsResponse> {
  const params = new URLSearchParams();
  if (opts.relations?.length) params.set('relations', opts.relations.join(','));
  if (opts.maxHops) params.set('max_hops', String(opts.maxHops));
  const qs = params.toString();
  return get(`${CORE}/memories/${encodeURIComponent(id)}/neighbors${qs ? `?${qs}` : ''}`);
}

export function getMemoryBacklinks(id: string, relation?: string): Promise<BacklinksResponse> {
  const qs = relation ? `?relation=${encodeURIComponent(relation)}` : '';
  return get(`${CORE}/memories/${encodeURIComponent(id)}/backlinks${qs}`);
}

export function getMemoryLinks(id: string): Promise<BacklinksResponse> {
  // Note: shape matches BacklinksResponse — `links` field rather than
  // `backlinks`. Use a permissive cast since callers handle both.
  return get(`${CORE}/memories/${encodeURIComponent(id)}/links`) as Promise<BacklinksResponse>;
}

export async function getMemoryMarkdown(id: string): Promise<string> {
  const res = await fetch(`${CORE}/memories/${encodeURIComponent(id)}/markdown`);
  if (!res.ok) throw new Error(`GET markdown ${id}: ${res.status}`);
  return res.text();
}

export async function putMemoryMarkdown(
  id: string,
  body: string,
): Promise<{ status: string; id: string; supersedes: string }> {
  const res = await fetch(`${CORE}/memories/${encodeURIComponent(id)}/markdown`, {
    method: 'PUT',
    headers: { 'Content-Type': 'text/markdown' },
    body,
  });
  if (!res.ok) throw new Error(`PUT markdown ${id}: ${res.status}`);
  return res.json();
}

export function getProjectDigest(project: string, days = 30): Promise<ProjectDigestByCategory> {
  return get(`${CORE}/projects/${encodeURIComponent(project)}/digest?days=${days}`);
}

export function getProjectAccumulators(project: string): Promise<WarmupDigest> {
  return get(`${CORE}/projects/${encodeURIComponent(project)}/accumulators`);
}

export function getSessionWarmup(
  sessionId: string,
  project?: string,
  topN = 15,
): Promise<WarmupDigest> {
  const params = new URLSearchParams();
  if (project) params.set('project', project);
  params.set('top_n', String(topN));
  return get(`${AGENT}/sessions/${encodeURIComponent(sessionId)}/warmup?${params}`);
}

export function getContext(
  project: string,
  query?: string,
  tokenBudget?: number,
): Promise<{ context: string }> {
  return post(`${CORE}/context`, { project, query, token_budget: tokenBudget });
}

export function getCoreMemory(project = 'global'): Promise<CoreMemory> {
  return get(`${CORE}/core/${encodeURIComponent(project)}`);
}

export function editCoreMemory(project: string, body: Omit<CoreEditRequest, 'project'>): Promise<CoreMemory> {
  return patch(`${CORE}/core/${encodeURIComponent(project)}`, body);
}

export function consolidate(): Promise<unknown> {
  return post(`${CORE}/consolidate`);
}

export function forgetGc(): Promise<unknown> {
  return post(`${CORE}/forget-gc`);
}

export interface ExportFilters {
  project?: string;
  sessionId?: string;
  obsType?: string;
  since?: string;
  until?: string;
  minImportance?: number;
}

export function getExport(filters: ExportFilters = {}): Promise<unknown> {
  const params = new URLSearchParams();
  if (filters.project) params.set('project', filters.project);
  if (filters.sessionId) params.set('session_id', filters.sessionId);
  if (filters.obsType) params.set('obs_type', filters.obsType);
  if (filters.since) params.set('since', filters.since);
  if (filters.until) params.set('until', filters.until);
  if (filters.minImportance) params.set('min_importance', String(filters.minImportance));
  const qs = params.toString();
  return get(qs ? `${CORE}/export?${qs}` : `${CORE}/export`);
}

// --- Agent Pipeline API ---

export function getSessions(limit = 20): Promise<{ sessions: Session[] }> {
  return get(`${AGENT}/sessions?limit=${limit}`);
}

export function getTimeline(sessionId: string, limit = 50): Promise<{ observations: Observation[] }> {
  return get(`${AGENT}/timeline?session_id=${encodeURIComponent(sessionId)}&limit=${limit}`);
}

export function getSessionTree(sessionId: string): Promise<{
  session: Session | null;
  runs: Run[];
  observations: Observation[];
}> {
  return get(`${AGENT}/sessions/${encodeURIComponent(sessionId)}/tree`);
}

export function searchRuns(
  query: string,
  project?: string,
  limit = 20,
): Promise<{ runs: Run[]; count: number }> {
  return post(`${AGENT}/runs`, { query, project, limit });
}

export function getRun(id: string): Promise<RunDetail> {
  return get(`${AGENT}/runs/${encodeURIComponent(id)}`);
}

export function getDigest(project?: string): Promise<ProjectDigest> {
  const qs = project ? `?project=${encodeURIComponent(project)}` : '';
  return get(`${AGENT}/digest${qs}`);
}

export function getCommits(project?: string, limit = 10, sessionId?: string, sha?: string): Promise<{ commits: Commit[] }> {
  const params = new URLSearchParams();
  if (project) params.set('project', project);
  if (sessionId) params.set('session_id', sessionId);
  if (sha) params.set('sha', sha);
  params.set('limit', String(limit));
  return get(`${AGENT}/commits?${params}`);
}

export function getCommitDiff(sha: string): Promise<{ sha: string; diff: string }> {
  return get(`${AGENT}/commits/${encodeURIComponent(sha)}/diff`);
}

export interface ObservationFilters {
  query?: string;
  project?: string;
  sessionId?: string;
  obsType?: string;
  since?: string;
  until?: string;
  minImportance?: number;
  limit?: number;
}

export function searchObservations(
  filters: ObservationFilters = {},
): Promise<{ observations: Observation[]; count: number }> {
  const params = new URLSearchParams();
  if (filters.query) params.set('query', filters.query);
  if (filters.project) params.set('project', filters.project);
  if (filters.sessionId) params.set('session_id', filters.sessionId);
  if (filters.obsType) params.set('obs_type', filters.obsType);
  if (filters.since) params.set('since', filters.since);
  if (filters.until) params.set('until', filters.until);
  if (filters.minImportance) params.set('min_importance', String(filters.minImportance));
  params.set('limit', String(filters.limit ?? 100));
  return get(`${AGENT}/observations?${params}`);
}

// --- Agent usage (generic, adapter-populated token records) ---

export interface UsageTotals {
  input: number;
  output: number;
  /** Raw token volume = input + output + cache_read + cache_creation. NOT cost-equivalent. */
  total: number;
  breakdown: Record<string, number>;
  cache_read: number;
  cache_creation: number;
  cache_hit_rate: number;
  /** Server-side billable-equivalent cost in USD, summed against the embedded
   * price table. Calls whose model isn't in the table contribute 0 and increment
   * `cost_unknown_calls`. */
  cost_usd: number;
  cost_unknown_calls: number;
  /** Auxiliary Anthropic calls Anthropic billed for but excluded from the JSONL
   * (ai-title, summary). Surfaced as a UI badge; never folded into token counts. */
  aux_calls: number;
}

export interface UsageCallRow {
  id: string;
  session_id: string;
  project: string;
  agent: string;
  model: string;
  external_id: string;
  timestamp: string;
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  prompt: string | null;
  tools: string[];
  breakdown: Record<string, number> | null;
  aux_calls: number | null;
}

export interface UsagePattern {
  id: string;
  kind: 'warning' | 'info' | 'neutral';
  title: string;
  body: string;
  action: string | null;
}

export interface DailyBucket {
  date: string;
  input: number;
  output: number;
  total: number;
  breakdown: Record<string, number>;
  calls: number;
  sessions: number;
}

export interface ModelBucket {
  model: string;
  total: number;
  input: number;
  output: number;
  calls: number;
}

export interface PromptRow {
  prompt: string;
  total: number;
  input: number;
  output: number;
  breakdown: Record<string, number>;
  session_id: string;
  model: string;
  date: string;
  call_count: number;
}

export interface UsageSessionRow {
  session_id: string;
  first_prompt: string | null;
  total: number;
  input: number;
  output: number;
  calls: number;
  model: string;
  date: string;
}

export interface SessionUsageView {
  session_id: string;
  totals: UsageTotals;
  model: string | null;
  primary_agent: string | null;
  call_count: number;
  calls: UsageCallRow[];
  patterns: UsagePattern[];
}

export interface ProjectUsageView {
  project: string;
  totals: UsageTotals;
  session_count: number;
  call_count: number;
  date_range: { from: string; to: string } | null;
  daily: DailyBucket[];
  models: ModelBucket[];
  top_prompts: PromptRow[];
  top_sessions: UsageSessionRow[];
  patterns: UsagePattern[];
}

export interface ProjectUsageFilters {
  from?: string;
  to?: string;
  model?: string;
}

export function fetchSessionUsage(sessionId: string): Promise<SessionUsageView> {
  return get(`${AGENT}/usage/session/${encodeURIComponent(sessionId)}`);
}

export function fetchProjectUsage(
  project: string,
  filters: ProjectUsageFilters = {},
): Promise<ProjectUsageView> {
  const params = new URLSearchParams();
  if (filters.from) params.set('from', filters.from);
  if (filters.to) params.set('to', filters.to);
  if (filters.model) params.set('model', filters.model);
  const qs = params.toString();
  const url = qs
    ? `${AGENT}/usage/project/${encodeURIComponent(project)}?${qs}`
    : `${AGENT}/usage/project/${encodeURIComponent(project)}`;
  return get(url);
}

/** Per-session token totals, uncapped. Optional project scopes to one
 *  project; omit for every session with usage data. Drives the Tokens
 *  column on the /sessions list. */
export function fetchSessionTotals(
  project?: string,
): Promise<{ sessions: UsageSessionRow[] }> {
  const url = project
    ? `${AGENT}/usage/sessions?project=${encodeURIComponent(project)}`
    : `${AGENT}/usage/sessions`;
  return get(url);
}

// --- atlas (corpus knowledge graph) ---
const ATLAS = '/api/v1/atlas';

export interface AtlasHubNode {
  id: string;
  label: string;
  kind: string;
  weighted_degree: number;
  clusters_touched: number;
}
export interface AtlasSurprisingLink {
  from: string;
  to: string;
  relation: string;
  score: number;
  surprise: number;
  why: string;
}
export interface AtlasIsolatedNode {
  id: string;
  label: string;
  kind: string;
  weighted_degree: number;
}
export interface AtlasInsights {
  nodes: number;
  edges: number;
  clusters: number;
  hub_nodes: AtlasHubNode[];
  surprising_links: AtlasSurprisingLink[];
  isolated_nodes: AtlasIsolatedNode[];
}
export interface AtlasGraph {
  nodes: Array<Record<string, unknown>>;
  edges: Array<Record<string, unknown>>;
}
export interface AtlasHit {
  id: string;
  kind: string;
  label: string;
  snippet?: string;
}

export interface AtlasStatus {
  project: string;
  running: boolean;
  step: string;
  last_report: unknown;
  error: string | null;
}

const pj = (p: string) => `project=${encodeURIComponent(p)}`;

export function getAtlasInsights(project: string): Promise<AtlasInsights> {
  return get(`${ATLAS}/insights?${pj(project)}`);
}
export function getAtlasGraph(project: string): Promise<AtlasGraph> {
  return get(`${ATLAS}/graph?${pj(project)}`);
}
export function atlasQuery(
  project: string,
  q: string,
  limit = 20,
): Promise<{ hits: AtlasHit[] }> {
  return get(`${ATLAS}/query?q=${encodeURIComponent(q)}&limit=${limit}&${pj(project)}`);
}

// Build endpoints are async: they return `{ started: true }` (or
// `{ error }` if a job is already running). Poll `atlasStatus`.
export function atlasStatus(project: string): Promise<AtlasStatus> {
  return get(`${ATLAS}/status?${pj(project)}`);
}
export function atlasBuildAll(
  project: string,
  src: { path?: string; git?: string; docs?: string },
): Promise<{ started?: boolean; error?: string }> {
  return post(`${ATLAS}/build?${pj(project)}`, { ...src, project });
}
export function atlasCode(
  project: string,
  src: { path?: string; git?: string },
): Promise<{ started?: boolean; error?: string }> {
  return post(`${ATLAS}/code?${pj(project)}`, { ...src, project });
}
export function atlasIngest(
  project: string,
  path: string,
): Promise<{ started?: boolean; error?: string }> {
  return post(`${ATLAS}/ingest?${pj(project)}`, { path, project });
}
export function atlasExtract(project: string): Promise<{ started?: boolean; error?: string }> {
  return post(`${ATLAS}/extract?${pj(project)}`);
}
export function atlasCluster(project: string): Promise<{ started?: boolean; error?: string }> {
  return post(`${ATLAS}/cluster?${pj(project)}`);
}
export async function atlasUpload(
  project: string,
  files: FileList | File[],
): Promise<{ started?: boolean; error?: string }> {
  const fd = new FormData();
  for (const f of Array.from(files)) fd.append('file', f, f.name);
  const res = await fetch(`${ATLAS}/upload?${pj(project)}`, { method: 'POST', body: fd });
  if (!res.ok) throw new Error(`POST ${ATLAS}/upload: ${res.status}`);
  return res.json();
}

// --- Renderer / replay (memdiff) ---
export function getRenderTokens(): Promise<RenderTokens> {
  return get(`${CORE}/render/tokens`);
}

export function getMemoryView(id: string): Promise<MemoryView> {
  return get(`${CORE}/memories/${encodeURIComponent(id)}/view`);
}

export function getReplays(): Promise<{ replays: ReplaySession[]; count: number }> {
  return get(`${CORE}/replays`);
}

export function getReplay(sessionId: string): Promise<ReplayDetail> {
  return get(`${CORE}/replays/${encodeURIComponent(sessionId)}`);
}
