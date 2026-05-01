import type { Observation } from '$lib/types';

export type CausalKind = 'causal-file' | 'causal-keyword' | 'causal-memory';

export interface CausalEdge {
  source: string;
  target: string;
  kind: CausalKind;
  weight: number;
  reason: string;
}

const WINDOW_MS = 5 * 60 * 1000; // 5 minute sliding window

function extractId(id: unknown): string {
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

/**
 * Infer causal edges between observations based on:
 *   - shared files       → solid (causal-file), strong signal
 *   - shared keywords    → dashed (causal-keyword), weak signal
 *   - shared memory ref  → dotted (causal-memory)
 *
 * Connects each observation to its earlier "parent" within a 5-minute window.
 * To keep the graph readable we only emit at most 2 inbound edges per node
 * (strongest by weight).
 */
export function inferEdges(observations: Observation[]): CausalEdge[] {
  if (observations.length < 2) return [];

  // Sort ascending by timestamp, defensively.
  const sorted = [...observations].sort(
    (a, b) => new Date(a.timestamp).getTime() - new Date(b.timestamp).getTime(),
  );

  const edges: CausalEdge[] = [];

  for (let i = 1; i < sorted.length; i++) {
    const cur = sorted[i];
    const curT = new Date(cur.timestamp).getTime();
    const curId = extractId(cur.id);
    const curFiles = new Set(cur.files ?? []);
    const curKeywords = new Set(cur.keywords ?? []);
    const curMemRefs = collectMemoryRefs(cur);

    const candidates: Array<{ edge: CausalEdge; weight: number }> = [];

    for (let j = i - 1; j >= 0; j--) {
      const prev = sorted[j];
      const prevT = new Date(prev.timestamp).getTime();
      if (curT - prevT > WINDOW_MS) break;

      const prevFiles = new Set(prev.files ?? []);
      const prevKeywords = new Set(prev.keywords ?? []);
      const prevMemRefs = collectMemoryRefs(prev);
      const prevId = extractId(prev.id);

      const sharedFiles = intersect(curFiles, prevFiles);
      if (sharedFiles.length > 0) {
        candidates.push({
          edge: {
            source: prevId,
            target: curId,
            kind: 'causal-file',
            weight: 10 + sharedFiles.length,
            reason: `shared files: ${sharedFiles.slice(0, 2).join(', ')}`,
          },
          weight: 10 + sharedFiles.length,
        });
        continue;
      }

      const sharedMems = intersect(curMemRefs, prevMemRefs);
      if (sharedMems.length > 0) {
        candidates.push({
          edge: {
            source: prevId,
            target: curId,
            kind: 'causal-memory',
            weight: 6,
            reason: `shared memory: ${sharedMems[0]}`,
          },
          weight: 6,
        });
        continue;
      }

      const sharedKws = intersect(curKeywords, prevKeywords);
      if (sharedKws.length >= 2) {
        candidates.push({
          edge: {
            source: prevId,
            target: curId,
            kind: 'causal-keyword',
            weight: 2 + sharedKws.length,
            reason: `shared keywords: ${sharedKws.slice(0, 2).join(', ')}`,
          },
          weight: 2 + sharedKws.length,
        });
      }
    }

    // Keep top 2 strongest inbound edges per node.
    candidates.sort((a, b) => b.weight - a.weight);
    for (const c of candidates.slice(0, 2)) {
      edges.push(c.edge);
    }
  }

  return edges;
}

function intersect<T>(a: Set<T>, b: Set<T>): T[] {
  const out: T[] = [];
  for (const x of a) if (b.has(x)) out.push(x);
  return out;
}

function collectMemoryRefs(obs: Observation): Set<string> {
  const refs = new Set<string>();
  const md = obs.metadata as Record<string, unknown> | null | undefined;
  if (md && Array.isArray(md.recalled_ids)) {
    for (const r of md.recalled_ids as unknown[]) {
      if (typeof r === 'string') refs.add(r);
    }
  }
  if (md && typeof md.memory_id === 'string') refs.add(md.memory_id);
  return refs;
}
