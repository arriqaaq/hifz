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
  Hifz,
} from './types';

const BASE = '/hifz';

async function get<T>(path: string): Promise<T> {
  const res = await fetch(`${BASE}${path}`);
  if (!res.ok) throw new Error(`GET ${path}: ${res.status}`);
  return res.json();
}

async function post<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${BASE}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) throw new Error(`POST ${path}: ${res.status}`);
  return res.json();
}

export function getHealth(): Promise<HealthResponse> {
  return get('/health');
}

export function getSessions(limit = 20): Promise<{ sessions: Session[] }> {
  return get(`/sessions?limit=${limit}`);
}

export function getTimeline(sessionId: string, limit = 50): Promise<{ observations: Observation[] }> {
  return get(`/timeline?session_id=${encodeURIComponent(sessionId)}&limit=${limit}`);
}

export function smartSearch(
  query: string,
  limit = 10,
  mode = 'hybrid',
  project?: string,
): Promise<{ results: SearchResult[]; count: number }> {
  return post('/smart-search', { query, limit, mode, project });
}

export function remember(body: RememberRequest): Promise<{ status: string; title: string }> {
  return post('/remember', body);
}

export function forget(id: string): Promise<{ status: string }> {
  return post('/forget', { id });
}

export function getCoreMemory(project = 'global'): Promise<CoreMemory> {
  return get(`/core?project=${encodeURIComponent(project)}`);
}

export function editCoreMemory(body: CoreEditRequest): Promise<CoreMemory> {
  return post('/core/edit', body);
}

export function searchRuns(
  query: string,
  project?: string,
  limit = 20,
): Promise<{ runs: Run[]; count: number }> {
  return post('/runs', { query, project, limit });
}

export function getDigest(project?: string): Promise<ProjectDigest> {
  const qs = project ? `?project=${encodeURIComponent(project)}` : '';
  return get(`/digest${qs}`);
}

export function getExport(): Promise<unknown> {
  return get('/export');
}

export function getCommits(project?: string, limit = 10, sessionId?: string, sha?: string): Promise<{ commits: Commit[] }> {
  const params = new URLSearchParams();
  if (project) params.set('project', project);
  if (sessionId) params.set('session_id', sessionId);
  if (sha) params.set('sha', sha);
  params.set('limit', String(limit));
  return get(`/commits?${params}`);
}

export function getCommitDiff(sha: string): Promise<{ sha: string; diff: string }> {
  return get(`/commits/${encodeURIComponent(sha)}/diff`);
}

export function consolidate(): Promise<unknown> {
  return post('/consolidate');
}

export function forgetGc(): Promise<unknown> {
  return post('/forget-gc');
}

export function getRun(id: string): Promise<RunDetail> {
  return get(`/run/${encodeURIComponent(id)}`);
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
  return get(`/observations?${params}`);
}

export function searchMemories(
  query?: string,
  limit = 50,
  project?: string,
  memType?: string,
): Promise<{ memories: Hifz[]; count: number }> {
  const params = new URLSearchParams();
  if (query) params.set('query', query);
  if (project) params.set('project', project);
  if (memType) params.set('mem_type', memType);
  params.set('limit', String(limit));
  return get(`/memories?${params}`);
}
