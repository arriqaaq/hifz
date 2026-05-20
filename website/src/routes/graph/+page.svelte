<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { getExport, getSessions, getAtlasGraph, listProjects } from '$lib/api';
  import CytoscapeGraph, {
    type GraphInputNode,
    type GraphInputEdge,
    type EdgeRel,
  } from '$lib/components/graph/CytoscapeGraph.svelte';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import FilterBar from '$lib/components/filters/FilterBar.svelte';
  import { type Filters, readFilters, applyFilters, toApiParams } from '$lib/stores/filters';
  import { goto } from '$app/navigation';
  import type { Observation, Memory, Run, Session } from '$lib/types';
  import EntityChip from '$lib/components/entity/EntityChip.svelte';
  import { extractId as extractEntityId } from '$lib/components/entity/entityHelpers';
  import { shell } from '$lib/stores/shell.svelte';
  import { Columns2, X as XIcon } from 'lucide-svelte';

  type RawNode = { id: string; kind: GraphInputNode['kind']; label: string };
  type RawEdge = { source: string; target: string; rel: EdgeRel };

  let nodes = $state<GraphInputNode[]>([]);
  let edges = $state<GraphInputEdge[]>([]);
  let allNodes = $state<GraphInputNode[]>([]);
  let allEdges = $state<GraphInputEdge[]>([]);
  let rawById = new Map<string, Observation | Memory | Run | Session>();
  let loading = $state(true);
  let error = $state('');
  let projects = $state<string[]>([]);
  let atlasProjects = $state<string[]>([]);
  let selectedId = $state<string | undefined>(undefined);
  let localMode = $state(false);
  let localDepth = $state(2);
  let lastKey = '';

  let filters = $derived<Filters>(readFilters(page.url));
  let split = $derived(page.url.searchParams.get('split') ?? '');
  // `Atlas` (corpus graph) is the default; `Activity` is the hifz
  // observation/session/run/memory graph. Lives in the URL so links/reloads
  // keep it (default `atlas` is omitted from the query string).
  let source = $derived<'atlas' | 'activity'>(
    page.url.searchParams.get('source') === 'activity' ? 'activity' : 'atlas',
  );
  // Project namespaces differ: Atlas uses slugs (`dst`), Activity uses hifz
  // session projects (full paths). Show the source-appropriate list.
  let projectOptions = $derived(source === 'atlas' ? atlasProjects : projects);

  function toggleSplit() {
    const url = new URL(page.url);
    if (split === 'observations') {
      url.searchParams.delete('split');
    } else {
      url.searchParams.set('split', 'observations');
    }
    void goto(`${url.pathname}?${url.searchParams.toString()}`, {
      replaceState: true,
      keepFocus: true,
      noScroll: true,
    });
  }

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
      if (source === 'atlas') {
        await loadAtlas(f.project);
        return;
      }
      const apiParams = toApiParams(f);
      const data = (await getExport({
        project: apiParams.project,
        sessionId: apiParams.sessionId,
        obsType: apiParams.obsType,
        since: apiParams.since,
        until: apiParams.until,
        minImportance: apiParams.minImportance,
      })) as Record<string, unknown>;

      // Build a lookup so node clicks can resolve back to the full record
      // without re-querying. Memories/observations/runs/sessions all carry
      // their own `id` field; we key the map on the canonical record string.
      rawById = new Map();
      const indexById = (rows: unknown, _label: string) => {
        if (!Array.isArray(rows)) return;
        for (const row of rows as Array<Record<string, unknown>>) {
          const id = extractId(row.id);
          if (id) rawById.set(id, row as unknown as Observation | Memory | Run | Session);
        }
      };
      indexById(data.observations, 'observation');
      indexById(data.memories, 'memory');
      indexById(data.runs, 'run');
      indexById(data.sessions, 'session');

      const rawNodes = (data.nodes ?? []) as RawNode[];
      const rawEdges = (data.edges ?? []) as RawEdge[];

      const ns: GraphInputNode[] = rawNodes.map((n) => {
        const raw = rawById.get(n.id);
        return {
          id: n.id,
          label: n.label,
          type: typeForNode(n, raw),
          kind: n.kind,
          keywords: (raw as { keywords?: string[] } | undefined)?.keywords,
          files: (raw as { files?: string[] } | undefined)?.files,
          timestamp: (raw as { timestamp?: string } | undefined)?.timestamp,
          obs_type: (raw as { obs_type?: string } | undefined)?.obs_type,
          importance: (raw as { importance?: number } | undefined)?.importance,
          raw,
        } satisfies GraphInputNode;
      });

      const es: GraphInputEdge[] = rawEdges.map((e) => ({
        source: e.source,
        target: e.target,
        rel: e.rel,
      }));

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

  // Atlas corpus graph — `atlas_node` / `atlas_edge` for the selected project
  // slug. Colors + shapes for the atlas kinds (document/concept/code_symbol/
  // external/file) already live in graphStyles.ts, keyed off `kind`/`type`.
  async function loadAtlas(project: string) {
    rawById = new Map();
    if (!project) {
      // Guard the reassignment: the $effect tracks `allNodes`, so handing it a
      // fresh empty array on every run would retrigger the effect in a loop.
      if (allNodes.length) allNodes = [];
      if (allEdges.length) allEdges = [];
      nodes = [];
      edges = [];
      return;
    }
    const data = await getAtlasGraph(project);
    const ns: GraphInputNode[] = data.nodes.map((n) => {
      const id = extractId((n as { id?: unknown }).id);
      return {
        id,
        label: String((n as { label?: unknown }).label ?? ''),
        kind: (n as { kind?: GraphInputNode['kind'] }).kind ?? 'concept',
        type: String((n as { kind?: unknown }).kind ?? 'other'),
        raw: n,
      } satisfies GraphInputNode;
    });
    const es: GraphInputEdge[] = data.edges.map((e) => ({
      source: extractId((e as { in?: unknown }).in),
      target: extractId((e as { out?: unknown }).out),
      rel: String((e as { relation?: unknown }).relation ?? ''),
    }));
    allNodes = ns;
    allEdges = es;
    applyLocalView();
  }

  function typeForNode(n: RawNode, raw: unknown): string {
    // The Cytoscape stylesheet keys node colour off `data(type)`. For
    // observations + commits we want the obs_type colour; for memories
    // the category; otherwise fall back to the kind itself.
    if (n.kind === 'observation' || n.kind === 'commit') {
      return ((raw as { obs_type?: string } | undefined)?.obs_type) ?? n.kind;
    }
    if (n.kind === 'memory') {
      return ((raw as { category?: string } | undefined)?.category) ?? 'memory';
    }
    return n.kind;
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
    // Activity projects come from hifz sessions; Atlas projects from the
    // atlas project registry (slugs) — union both so neither namespace's
    // projects go missing from the dropdown.
    try {
      const r = await getSessions(200);
      const seen = new Set<string>();
      for (const s of r.sessions) if (s.project && !seen.has(s.project)) seen.add(s.project);
      projects = Array.from(seen).sort();
    } catch {
      // ignore
    }
    try {
      const r = await listProjects();
      atlasProjects = r.projects.map((p) => p.slug).sort();
    } catch {
      // ignore
    }
  });

  $effect(() => {
    // Reload when either the filters (URL search) or the source change.
    const key = `${source}|${page.url.search}`;
    if (key === lastKey && allNodes.length > 0) return;
    lastKey = key;
    void load(filters);
  });

  function onSelect(n: GraphInputNode | null) {
    if (!n) {
      shell.closeDrawer();
      if (localMode) applyLocalView();
      return;
    }
    if (n.kind === 'memory' && n.raw) {
      shell.openDrawer({ kind: 'memory', id: n.id, data: n.raw as Memory });
    } else if ((n.kind === 'observation' || n.kind === 'commit') && n.raw) {
      shell.openDrawer({
        kind: 'observation',
        id: n.id,
        data: n.raw as Observation,
        onFilterToSession: filterToSession,
      });
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

  // `applyFilters` rebuilds the query string from filters alone and would drop
  // our `source` param, so navigate through a helper that re-appends it.
  function navigate(f: Filters, src: 'atlas' | 'activity') {
    const p = new URLSearchParams();
    if (f.query) p.set('q', f.query);
    if (f.sessionId) p.set('session', f.sessionId);
    if (f.project) p.set('project', f.project);
    if (f.obsTypes.length) p.set('type', f.obsTypes.join(','));
    if (f.since) p.set('since', f.since);
    if (f.until) p.set('until', f.until);
    if (f.minImportance > 0) p.set('min_importance', String(f.minImportance));
    if (split) p.set('split', split);
    if (src !== 'atlas') p.set('source', src); // default `atlas` stays implicit
    const qs = p.toString();
    void goto(qs ? `${page.url.pathname}?${qs}` : page.url.pathname, {
      replaceState: true,
      keepFocus: true,
      noScroll: true,
    });
  }

  function onFiltersChange(next: Filters) {
    navigate(next, source);
  }

  function setSource(src: 'atlas' | 'activity') {
    if (src === source) return;
    // Project slugs (Atlas) and session paths (Activity) are different
    // namespaces — clear the selection when switching.
    navigate({ ...filters, project: '' }, src);
  }

  function filterToSession(sid: string) {
    void applyFilters({ ...filters, sessionId: sid });
    shell.closeDrawer();
  }
</script>

<div class="page">
  <div class="source-toggle" role="group" aria-label="Graph source">
    <button
      type="button"
      class="source-btn"
      class:active={source === 'atlas'}
      onclick={() => setSource('atlas')}
    >
      Atlas
    </button>
    <button
      type="button"
      class="source-btn"
      class:active={source === 'activity'}
      onclick={() => setSource('activity')}
    >
      Activity
    </button>
  </div>

  <FilterBar {filters} projects={projectOptions} onChange={onFiltersChange} />

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
    <button type="button" class="split-btn" onclick={toggleSplit} title="Toggle split with observations">
      {#if split === 'observations'}
        <XIcon size={12} strokeWidth={1.6} />
        Close split
      {:else}
        <Columns2 size={12} strokeWidth={1.6} />
        Split: observations
      {/if}
    </button>
    <span class="stat">{nodes.length} nodes · {edges.length} edges{#if allNodes.length !== nodes.length} (of {allNodes.length}){/if}</span>
  </div>

  <div class="layout" class:split={split === 'observations'}>
    <div class="graph-area">
      {#if loading}
        <LoadingSpinner />
      {:else if error}
        <div class="card" style="border-color: var(--danger);">
          <p style="color: var(--danger); margin: 0;">{error}</p>
        </div>
      {:else if nodes.length === 0}
        {#if source === 'atlas' && !filters.project}
          <p class="empty">Select an Atlas project to visualize its corpus graph.</p>
        {:else}
          <p class="empty">No data to visualize. Try clearing filters or check the server is running.</p>
        {/if}
      {:else}
        <CytoscapeGraph {nodes} {edges} bind:selectedId {onSelect} {onExpand} />
      {/if}
    </div>

    {#if split === 'observations'}
      <aside class="split-pane">
        <h3 class="pane-h">Observations ({nodes.filter((n) => n.kind === 'observation').length})</h3>
        <ol class="obs-list">
          {#each nodes.filter((n) => n.kind === 'observation') as n}
            <li class="obs-list-item" class:selected={selectedId === n.id}>
              <button
                type="button"
                class="obs-list-row"
                onclick={() => {
                  selectedId = n.id;
                  if (n.raw) {
                    shell.openDrawer({
                      kind: 'observation',
                      id: n.id,
                      data: n.raw as Observation,
                      onFilterToSession: filterToSession,
                    });
                  }
                }}
              >
                <EntityChip kind="observation" id={n.id} label={n.obs_type ?? 'obs'} size="sm" href={null} />
                {#if (n.raw as Observation | undefined)?.session_id}
                  {@const sid = extractEntityId((n.raw as Observation).session_id)}
                  <EntityChip kind="session" id={sid} size="sm" />
                {/if}
                <span class="obs-list-title">{n.label}</span>
              </button>
            </li>
          {/each}
        </ol>
      </aside>
    {/if}
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 100px);
  }

  .source-toggle {
    display: inline-flex;
    align-self: flex-start;
    border: 1px solid var(--line-strong);
    margin-bottom: 12px;
  }
  .source-btn {
    padding: 5px 16px;
    font-family: var(--font-ui);
    font-size: 11px;
    font-weight: 600;
    background: var(--bg);
    border: none;
    color: var(--ink-muted);
    cursor: pointer;
  }
  .source-btn + .source-btn {
    border-left: 1px solid var(--line-strong);
  }
  .source-btn.active {
    background: var(--neon);
    color: var(--ink);
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

  .split-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    font-size: 11px;
    font-family: var(--font-ui);
    color: var(--ink-secondary);
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .split-btn:hover {
    background: var(--surface-alt);
    border-color: var(--line-strong);
  }

  .layout {
    flex: 1;
    display: grid;
    grid-template-columns: 1fr;
    gap: 12px;
    min-height: 0;
  }
  .layout.split {
    grid-template-columns: 1.3fr 1fr;
  }

  .split-pane {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 10px 12px;
    overflow-y: auto;
    box-shadow: var(--shadow-sm);
  }

  .pane-h {
    margin: 0 0 8px;
    font-family: var(--font-ui);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-faint);
  }

  .obs-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .obs-list-item.selected .obs-list-row {
    background: color-mix(in srgb, var(--neon) 22%, transparent);
  }
  .obs-list-row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
    width: 100%;
    border-radius: var(--radius-sm);
    flex-wrap: wrap;
  }
  .obs-list-row:hover { background: var(--surface-alt); }
  .obs-list-title {
    flex: 1;
    font-family: var(--font-ui);
    font-size: 11.5px;
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    border: 1px solid var(--line-strong);
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
