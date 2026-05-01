import { Layers, Workflow, Activity, Brain, GitCommit, Folder, type Icon } from 'lucide-svelte';

export type EntityKind = 'session' | 'run' | 'observation' | 'memory' | 'commit' | 'project';

export interface EntityRef {
  kind: EntityKind;
  id: string;
  label?: string;
}

export const KIND_COLOR: Record<EntityKind, string> = {
  session: 'var(--c-session)',
  run: 'var(--c-run)',
  observation: 'var(--c-obs)',
  memory: 'var(--c-memory)',
  commit: 'var(--c-commit)',
  project: 'var(--c-project)',
};

export const KIND_ICON: Record<EntityKind, typeof Icon> = {
  session: Layers as unknown as typeof Icon,
  run: Workflow as unknown as typeof Icon,
  observation: Activity as unknown as typeof Icon,
  memory: Brain as unknown as typeof Icon,
  commit: GitCommit as unknown as typeof Icon,
  project: Folder as unknown as typeof Icon,
};

export function extractId(id: unknown): string {
  if (typeof id === 'string') return id;
  if (id && typeof id === 'object') {
    const o = id as Record<string, unknown>;
    if (typeof o.key === 'string') return o.key;
    if (o.key && typeof o.key === 'object' && 'String' in (o.key as Record<string, unknown>)) {
      return (o.key as { String: string }).String;
    }
  }
  return String(id);
}

export function shortId(id: string): string {
  if (!id) return '';
  const tail = id.replace(/^(session|run|observation|memory):/, '');
  return tail.length > 12 ? tail.slice(0, 10) + '…' : tail;
}

export function kindForObsType(obsType: string): EntityKind {
  return obsType === 'commit_made' ? 'commit' : 'observation';
}

export function entityHref(kind: EntityKind, id: string): string | null {
  const tail = id.replace(/^[^:]+:/, '');
  if (kind === 'session') return `/sessions/${encodeURIComponent(tail)}`;
  if (kind === 'run') return `/runs/${encodeURIComponent(tail)}`;
  if (kind === 'commit') return `/commits/${encodeURIComponent(tail)}`;
  return null;
}
