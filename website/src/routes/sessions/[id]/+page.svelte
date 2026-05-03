<script lang="ts">
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import { getSessionTree } from '$lib/api';
  import type { Session, Run, Observation } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import Waterfall from '$lib/components/timeline/Waterfall.svelte';
  import TraceTree from '$lib/components/timeline/TraceTree.svelte';
  import CytoscapeGraph, {
    type GraphInputNode,
    type GraphInputEdge,
  } from '$lib/components/graph/CytoscapeGraph.svelte';
  import { inferEdges, type CausalEdge } from '$lib/timeline/causality';
  import { extractId } from '$lib/components/entity/entityHelpers';
  import EntityChip from '$lib/components/entity/EntityChip.svelte';
  import { shell } from '$lib/stores/shell.svelte';

  let session = $state<Session | null>(null);
  let runs = $state<Run[]>([]);
  let observations = $state<Observation[]>([]);
  let loading = $state(true);
  let error = $state('');
  let selectedId = $state<string | undefined>(undefined);
  let activeTab = $state<'timeline' | 'list'>('timeline');

  let sessionId = $derived(decodeURIComponent(page.params.id ?? ''));
  let timelineId = $derived(sessionId.replace(/^session:/, ''));

  onMount(async () => {
    try {
      const res = await getSessionTree(timelineId);
      session = res.session;
      runs = res.runs ?? [];
      observations = res.observations ?? [];
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load';
    } finally {
      loading = false;
    }
  });

  let causalEdges = $derived<CausalEdge[]>(inferEdges(observations));

  let graphNodes = $derived.by<GraphInputNode[]>(() => {
    return observations
      .filter((o) => o.title !== 'unknown call' && o.obs_type !== 'conversation')
      .map((o) => ({
        id: extractId(o.id),
        label: o.title,
        type: o.obs_type,
        kind: 'observation' as const,
        keywords: o.keywords,
        files: o.files,
        timestamp: o.timestamp,
        obs_type: o.obs_type,
        importance: o.importance,
        raw: o,
      }));
  });

  let graphEdges = $derived<GraphInputEdge[]>(
    causalEdges.map((e) => ({ source: e.source, target: e.target, kind: e.kind })),
  );

  let summary = $derived.by(() => {
    if (observations.length === 0) return null;
    const first = observations[0];
    const last = observations[observations.length - 1];
    const start = new Date(first.timestamp).getTime();
    const end = new Date(last.timestamp).getTime();
    const ms = Math.max(0, end - start);
    const mins = Math.floor(ms / 60000);
    return {
      count: observations.length,
      runs: runs.length,
      start: new Date(first.timestamp),
      end: new Date(last.timestamp),
      durationLabel: mins < 60 ? `${mins}m` : `${Math.floor(mins / 60)}h ${mins % 60}m`,
    };
  });

  function openObsDrawer(obs: Observation) {
    shell.openDrawer({ kind: 'observation', id: extractId(obs.id), data: obs });
  }

  function onWaterfallSelect(obs: Observation | null) {
    if (obs) {
      selectedId = extractId(obs.id);
      openObsDrawer(obs);
    } else {
      selectedId = undefined;
    }
  }

  function onGraphSelect(n: GraphInputNode | null) {
    if (!n) return;
    selectedId = n.id;
    if (n.raw) openObsDrawer(n.raw as Observation);
  }

  function onTreeSelect(kind: 'session' | 'run' | 'observation', id: string, data: unknown) {
    selectedId = id;
    if (kind === 'observation') {
      openObsDrawer(data as Observation);
    }
  }

  function fmt(d: Date): string {
    return d.toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }
</script>

<div class="page">
  {#if loading}
    <LoadingSpinner />
  {:else if error}
    <div class="card" style="border-color: var(--danger);">
      <p style="color: var(--danger); margin: 0;">{error}</p>
    </div>
  {:else if !session}
    <p class="empty">Session not found.</p>
  {:else}
    <header class="head">
      <div class="head-top">
        <EntityChip kind="session" id={extractId(session.id)} size="sm" href={null} />
        <h2 class="title">{session.name ?? session.project ?? 'Session'}</h2>
        {#if session.project}
          <span class="badge badge-cyan">{session.project}</span>
        {/if}
      </div>
      {#if summary}
        <div class="meta">
          <span>{fmt(summary.start)} → {fmt(summary.end)}</span>
          <span class="dot">·</span>
          <span>{summary.durationLabel}</span>
          <span class="dot">·</span>
          <span>{summary.runs} runs · {summary.count} observations</span>
          <span class="dot">·</span>
          <span>{causalEdges.length} causal links</span>
        </div>
      {/if}
    </header>

    <div class="layout">
      <aside class="tree-pane">
        <h3 class="pane-h">Trace</h3>
        <TraceTree
          {session}
          {runs}
          {observations}
          {selectedId}
          onSelect={onTreeSelect}
        />
      </aside>

      <section class="main-pane">
        <div class="tabs">
          <button class="tab" class:active={activeTab === 'timeline'} onclick={() => (activeTab = 'timeline')}>
            Timeline
          </button>
          <button class="tab" class:active={activeTab === 'list'} onclick={() => (activeTab = 'list')}>
            List
          </button>
        </div>

        {#if activeTab === 'timeline'}
          <div class="timeline-section">
            <h3 class="pane-h">Time axis</h3>
            <Waterfall
              {observations}
              edges={causalEdges}
              bind:selectedId
              onSelect={onWaterfallSelect}
            />
          </div>
          <div class="graph-section">
            <h3 class="pane-h">Causal graph</h3>
            <div class="graph-area">
              <CytoscapeGraph
                nodes={graphNodes}
                edges={graphEdges}
                bind:selectedId
                onSelect={onGraphSelect}
                compact={true}
              />
            </div>
          </div>
        {:else}
          <ol class="list">
            {#each observations as obs}
              {@const oid = extractId(obs.id)}
              <li class="list-item" class:selected={selectedId === oid}>
                <button type="button" class="list-row" onclick={() => onWaterfallSelect(obs)}>
                  <span class="list-time">{new Date(obs.timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}</span>
                  <EntityChip kind="observation" id={oid} label={obs.obs_type} size="sm" href={null} />
                  <span class="list-title">{obs.title}</span>
                  {#if obs.importance >= 7}<span class="list-imp">★ {obs.importance}</span>{/if}
                </button>
              </li>
            {/each}
          </ol>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .head {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .head-top {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .title {
    margin: 0;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .meta {
    display: flex;
    gap: 6px;
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-size: 11px;
    color: var(--ink-muted);
    flex-wrap: wrap;
  }
  .dot { color: var(--ink-faint); }

  .layout {
    display: grid;
    grid-template-columns: 360px 1fr;
    gap: 16px;
    align-items: start;
    min-height: calc(100vh - 200px);
  }

  .tree-pane,
  .main-pane {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 12px;
    box-shadow: var(--shadow-sm);
  }

  .tree-pane {
    max-height: calc(100vh - 200px);
    overflow-y: auto;
    position: sticky;
    top: 16px;
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

  .tabs {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    margin-bottom: 12px;
    overflow: hidden;
  }
  .tab {
    padding: 5px 14px;
    background: var(--surface);
    border: none;
    font-family: var(--font-ui);
    font-size: 11px;
    color: var(--ink-muted);
    cursor: pointer;
  }
  .tab:not(:last-child) {
    border-right: 1px solid var(--line);
  }
  .tab.active {
    background: var(--ink);
    color: var(--surface);
  }

  .timeline-section,
  .graph-section {
    margin-bottom: 16px;
  }

  .graph-area {
    height: 360px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .list-item.selected .list-row {
    background: color-mix(in srgb, var(--neon) 22%, transparent);
  }
  .list-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    cursor: pointer;
    width: 100%;
    text-align: left;
  }
  .list-row:hover {
    background: var(--surface-alt);
  }
  .list-time {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-size: 11px;
    color: var(--ink-muted);
    min-width: 70px;
  }
  .list-title {
    flex: 1;
    font-family: var(--font-ui);
    font-size: 12px;
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .list-imp {
    font-size: 11px;
    color: var(--c-obs);
    font-family: var(--font-mono);
  }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }
</style>
