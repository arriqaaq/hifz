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
): Promise<{ memories: Memory[]; count: number }> {
  const params = new URLSearchParams();
  if (query) params.set('query', query);
  if (project) params.set('project', project);
  if (category) params.set('category', category);
  params.set('limit', String(limit));
  return get(`${CORE}/memories?${params}`);
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

export function getExport(): Promise<unknown> {
  return get(`${CORE}/export`);
}

// --- Agent Pipeline API ---

export function getSessions(limit = 20): Promise<{ sessions: Session[] }> {
  return get(`${AGENT}/sessions?limit=${limit}`);
}

export function getTimeline(sessionId: string, limit = 50): Promise<{ observations: Observation[] }> {
  return get(`${AGENT}/timeline?session_id=${encodeURIComponent(sessionId)}&limit=${limit}`);
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

export function searchObservations(
  query?: string,
  limit = 100,
  project?: string,
  sessionId?: string,
): Promise<{ observations: Observation[]; count: number }> {
  const params = new URLSearchParams();
  if (query) params.set('query', query);
  if (project) params.set('project', project);
  if (sessionId) params.set('session_id', sessionId);
  params.set('limit', String(limit));
  return get(`${AGENT}/observations?${params}`);
}
