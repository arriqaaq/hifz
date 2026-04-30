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
  keywords: string[];
  files: string[];
  importance: number;
  confidence: number | null;
  metadata?: Record<string, unknown> | null;
}

export interface Memory {
  id: string;
  project: string;
  category: string;
  title: string;
  content: string;
  keywords: string[];
  files: string[];
  tags: string[];
  context: string | null;
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
  project?: string;
  session_id?: string;
}

export interface CoreEditRequest {
  project: string;
  field: 'identity' | 'goals' | 'invariants' | 'watchlist';
  op: 'set' | 'add' | 'remove';
  value: string;
}
