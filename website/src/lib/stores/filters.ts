import { goto } from '$app/navigation';
import { page } from '$app/state';

// Filter state lives in URL search params — single source of truth, shareable links.
// Helpers below parse/serialize the URL.

export interface Filters {
  query: string;
  sessionId: string;
  project: string;
  obsTypes: string[];
  since: string;       // YYYY-MM-DD or ''
  until: string;       // YYYY-MM-DD or ''
  minImportance: number; // 0..10, 0 = no filter
}

export const EMPTY: Filters = {
  query: '',
  sessionId: '',
  project: '',
  obsTypes: [],
  since: '',
  until: '',
  minImportance: 0,
};

export function readFilters(url: URL): Filters {
  const p = url.searchParams;
  const types = p.get('type');
  const minImp = Number(p.get('min_importance') ?? '0');
  return {
    query: p.get('q') ?? '',
    sessionId: p.get('session') ?? '',
    project: p.get('project') ?? '',
    obsTypes: types ? types.split(',').filter(Boolean) : [],
    since: p.get('since') ?? '',
    until: p.get('until') ?? '',
    minImportance: Number.isFinite(minImp) ? minImp : 0,
  };
}

function writeParams(f: Filters): URLSearchParams {
  const p = new URLSearchParams();
  if (f.query) p.set('q', f.query);
  if (f.sessionId) p.set('session', f.sessionId);
  if (f.project) p.set('project', f.project);
  if (f.obsTypes.length) p.set('type', f.obsTypes.join(','));
  if (f.since) p.set('since', f.since);
  if (f.until) p.set('until', f.until);
  if (f.minImportance > 0) p.set('min_importance', String(f.minImportance));
  return p;
}

export async function applyFilters(f: Filters): Promise<void> {
  const params = writeParams(f);
  const qs = params.toString();
  const target = qs ? `${page.url.pathname}?${qs}` : page.url.pathname;
  await goto(target, { replaceState: true, keepFocus: true, noScroll: true });
}

export function isEmpty(f: Filters): boolean {
  return (
    !f.query &&
    !f.sessionId &&
    !f.project &&
    f.obsTypes.length === 0 &&
    !f.since &&
    !f.until &&
    f.minImportance === 0
  );
}

// Convert filter object → params for backend API calls.
export function toApiParams(f: Filters): {
  query?: string;
  sessionId?: string;
  project?: string;
  obsType?: string;
  since?: string;
  until?: string;
  minImportance?: number;
} {
  const out: Record<string, unknown> = {};
  if (f.query) out.query = f.query;
  if (f.sessionId) out.sessionId = f.sessionId;
  if (f.project) out.project = f.project;
  if (f.obsTypes.length) out.obsType = f.obsTypes.join(',');
  if (f.since) out.since = toRfc3339Start(f.since);
  if (f.until) out.until = toRfc3339End(f.until);
  if (f.minImportance > 0) out.minImportance = f.minImportance;
  return out;
}

function toRfc3339Start(date: string): string {
  return `${date}T00:00:00Z`;
}
function toRfc3339End(date: string): string {
  return `${date}T23:59:59Z`;
}

export const OBS_TYPES = [
  'file_edit',
  'file_read',
  'file_write',
  'command_run',
  'search',
  'commit_made',
  'conversation',
  'other',
] as const;

export type ObsType = (typeof OBS_TYPES)[number];

export const DATE_PRESETS: Array<{ label: string; days: number }> = [
  { label: 'Today', days: 0 },
  { label: '7d', days: 7 },
  { label: '30d', days: 30 },
];

export function presetRange(days: number): { since: string; until: string } {
  const now = new Date();
  const until = now.toISOString().slice(0, 10);
  const start = new Date(now);
  start.setDate(start.getDate() - days);
  return { since: start.toISOString().slice(0, 10), until };
}
