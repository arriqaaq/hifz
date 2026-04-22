export interface HealthResponse {
  status: string;
  version: string;
  sessions: number;
  observations: number;
  memories: number;
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
  session_id: string | null;
  timestamp: string;
  obs_type: string;
  title: string;
  subtitle: string | null;
  facts: string[];
  narrative: string;
  concepts: string[];
  files: string[];
  importance: number;
  confidence: number | null;
}

export interface Hifz {
  id: string;
  project: string;
  mem_type: string;
  title: string;
  content: string;
  concepts: string[];
  files: string[];
  keywords: string[];
  tags: string[];
  context: string | null;
  strength: number;
  access_count: number;
  last_accessed_at: string;
  version: number;
  parent_id: string | null;
  supersedes: string[] | null;
  is_latest: boolean;
  forget_after: string | null;
  created_at: string;
  updated_at: string;
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
  lesson: string | null;
  commit_id?: string | RecordId | null;
  plan_id?: string | RecordId | null;
}

export interface RunDetail {
  run: Run;
  observations: Observation[];
}

export interface RecordId {
  table: string;
  key: { String?: string; Number?: number } | string;
}

export interface Commit {
  sha: string;
  message: string;
  author: string;
  branch: string;
  project: string;
  files_changed: string[];
  insertions: number | null;
  deletions: number | null;
  session_id: string | null;
  run_id: string | null;
  timestamp: string;
}

export interface ConceptFreq {
  concept: string;
  frequency: number;
}

export interface FileFreq {
  file: string;
  frequency: number;
}

export interface ProjectDigest {
  project: string;
  updated_at: string;
  top_concepts: ConceptFreq[];
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
}

export interface RememberRequest {
  title: string;
  content: string;
  type?: string;
  concepts?: string[];
  files?: string[];
  project?: string;
}

export interface CoreEditRequest {
  project: string;
  field: 'identity' | 'goals' | 'invariants' | 'watchlist';
  op: 'set' | 'add' | 'remove';
  value: string;
}
