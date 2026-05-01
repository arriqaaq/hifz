<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { getExport, getSessions } from '$lib/api';
  import CytoscapeGraph, {
    type GraphInputNode,
    type GraphInputEdge,
  } from '$lib/components/graph/CytoscapeGraph.svelte';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import FilterBar from '$lib/components/filters/FilterBar.svelte';
  import DetailDrawer from '$lib/components/common/DetailDrawer.svelte';
  import { type Filters, readFilters, applyFilters, toApiParams } from '$lib/stores/filters';
  import type { Observation, Memory } from '$lib/types';

  let nodes = $state<GraphInputNode[]>([]);
  let edges = $state<GraphInputEdge[]>([]);
  let allNodes = $state<GraphInputNode[]>([]);
  let allEdges = $state<GraphInputEdge[]>([]);
  let loading = $state(true);
  let error = $state('');
  let projects = $state<string[]>([]);
  let selectedId = $state<string | undefined>(undefined);
  let drawerItem = $state<
    | { kind: 'observation'; data: Observation }
    | { kind: 'memory'; data: Memory }
    | null
  >(null);
  let localMode = $state(false);
  let localDepth = $state(2);
  let lastSearch = '';

  let filters = $derived<Filters>(readFilters(page.url));

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

  async function load(f: Filters) {
    loading = true;
    error = '';
    try {
      const apiParams = toApiParams(f);
      const data = (await getExport({
        project: apiParams.project,
        sessionId: apiParams.sessionId,
        obsType: apiParams.obsType,
        since: apiParams.since,
        until: apiParams.until,
        minImportance: apiParams.minImportance,
      })) as Record<string, unknown[]>;

      const memories = (data.memories ?? []) as Memory[];
      const observations = (data.observations ?? []) as Observation[];

      const ns: GraphInputNode[] = [];
      const conceptMap = new Map<string, string[]>();

      for (const m of memories) {
        const id = extractId(m.id);
        ns.push({
          id,
          label: m.title,
          type: m.category,
          kind: 'memory',
          keywords: m.keywords,
          files: m.files,
          raw: m,
        });
        for (const c of m.keywords ?? []) {
          if (!conceptMap.has(c)) conceptMap.set(c, []);
          conceptMap.get(c)!.push(id);
        }
      }

      for (const o of observations) {
        if (o.title === 'unknown call' || o.obs_type === 'conversation') continue;
        const id = extractId(o.id);
        ns.push({
          id,
          label: o.title,
          type: o.obs_type,
          kind: 'observation',
          keywords: o.keywords,
          files: o.files,
          timestamp: o.timestamp,
          obs_type: o.obs_type,
          importance: o.importance,
          raw: o,
        });
        for (const c of o.keywords ?? []) {
          if (!conceptMap.has(c)) conceptMap.set(c, []);
          conceptMap.get(c)!.push(id);
        }
      }

      // Build edges from shared concepts
      const edgeSet = new Set<string>();
      const es: GraphInputEdge[] = [];
      for (const indices of conceptMap.values()) {
        if (indices.length > 20) continue;
        for (let i = 0; i < indices.length; i++) {
          for (let j = i + 1; j < indices.length; j++) {
            const a = indices[i];
            const b = indices[j];
            const key = a < b ? `${a}~${b}` : `${b}~${a}`;
            if (!edgeSet.has(key)) {
              edgeSet.add(key);
              es.push({ source: a, target: b });
            }
          }
        }
      }

      allNodes = ns;
      allEdges = es;
      applyLocalView();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load';
      allNodes = [];
      allEdges = [];
      nodes = [];
      edges = [];
    } finally {
      loading = false;
    }
  }

  function applyLocalView() {
    if (!localMode || !selectedId) {
      nodes = allNodes;
      edges = allEdges;
      return;
    }
    const adj = new Map<string, Set<string>>();
    for (const e of allEdges) {
      if (!adj.has(e.source)) adj.set(e.source, new Set());
      if (!adj.has(e.target)) adj.set(e.target, new Set());
      adj.get(e.source)!.add(e.target);
      adj.get(e.target)!.add(e.source);
    }
    const visited = new Set<string>([selectedId]);
    let frontier = new Set<string>([selectedId]);
    for (let d = 0; d < localDepth; d++) {
      const next = new Set<string>();
      for (const id of frontier) {
        const nbrs = adj.get(id);
        if (!nbrs) continue;
        for (const n of nbrs) {
          if (!visited.has(n)) {
            visited.add(n);
            next.add(n);
          }
        }
      }
      frontier = next;
    }
    nodes = allNodes.filter((n) => visited.has(n.id));
    edges = allEdges.filter((e) => visited.has(e.source) && visited.has(e.target));
  }

  onMount(async () => {
    try {
      const r = await getSessions(200);
      const seen = new Set<string>();
      for (const s of r.sessions) if (s.project && !seen.has(s.project)) seen.add(s.project);
      projects = Array.from(seen).sort();
    } catch {
      // ignore
    }
  });

  $effect(() => {
    const cur = page.url.search;
    if (cur === lastSearch && allNodes.length > 0) return;
    lastSearch = cur;
    void load(filters);
  });

  function onSelect(n: GraphInputNode | null) {
    if (!n) {
      drawerItem = null;
      if (localMode) applyLocalView();
      return;
    }
    if (n.kind === 'memory') {
      drawerItem = { kind: 'memory', data: n.raw as Memory };
    } else {
      drawerItem = { kind: 'observation', data: n.raw as Observation };
    }
    if (localMode) applyLocalView();
  }

  function onExpand(n: GraphInputNode) {
    // Re-root local view at the double-clicked node.
    selectedId = n.id;
    localMode = true;
    applyLocalView();
  }

  function toggleLocal() {
    localMode = !localMode;
    applyLocalView();
  }

  function onDepthChange(e: Event) {
    localDepth = Number((e.currentTarget as HTMLInputElement).value);
    applyLocalView();
  }

  function onFiltersChange(next: Filters) {
    void applyFilters(next);
  }

  function filterToSession(sid: string) {
    void applyFilters({ ...filters, sessionId: sid });
    drawerItem = null;
  }
</script>

<div class="page">
  <FilterBar {filters} {projects} onChange={onFiltersChange} />

  <div class="graph-controls">
    <label class="local-toggle">
      <input type="checkbox" checked={localMode} onchange={toggleLocal} />
      Local mode
    </label>
    {#if localMode}
      <span class="depth">
        depth
        <input type="range" min="1" max="4" value={localDepth} oninput={onDepthChange} />
        <span class="depth-val">{localDepth}</span>
      </span>
      {#if !selectedId}
        <span class="hint-inline">click a node to set the root</span>
      {/if}
    {/if}
    <span class="stat">{nodes.length} nodes · {edges.length} edges{#if allNodes.length !== nodes.length} (of {allNodes.length}){/if}</span>
  </div>

  <div class="graph-area">
    {#if loading}
      <LoadingSpinner />
    {:else if error}
      <div class="card" style="border-color: var(--accent);">
        <p style="color: var(--accent); margin: 0;">{error}</p>
      </div>
    {:else if nodes.length === 0}
      <p class="empty">No data to visualize. Try clearing filters or check the server is running.</p>
    {:else}
      <CytoscapeGraph {nodes} {edges} bind:selectedId {onSelect} {onExpand} />
    {/if}
  </div>

  <DetailDrawer
    item={drawerItem}
    onClose={() => (drawerItem = null)}
    onFilterToSession={filterToSession}
  />
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 100px);
  }

  .graph-controls {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 6px 0 10px;
    flex-wrap: wrap;
  }

  .local-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-family: var(--font-ui);
    color: var(--ink);
    cursor: pointer;
  }

  .depth {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-family: var(--font-ui);
    color: var(--ink-muted);
  }
  .depth input[type='range'] { width: 80px; }
  .depth-val { font-family: var(--font-mono); color: var(--ink); min-width: 14px; text-align: center; }

  .hint-inline {
    font-size: 10px;
    font-family: var(--font-ui);
    color: var(--ink-faint);
    font-style: italic;
  }

  .stat {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
  }

  .graph-area {
    flex: 1;
    position: relative;
    border: 1px solid var(--border);
    overflow: hidden;
  }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }
</style>
