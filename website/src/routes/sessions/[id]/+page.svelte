<script lang="ts">
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import { getTimeline } from '$lib/api';
  import type { Observation } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import Waterfall from '$lib/components/timeline/Waterfall.svelte';
  import CytoscapeGraph, {
    type GraphInputNode,
    type GraphInputEdge,
  } from '$lib/components/graph/CytoscapeGraph.svelte';
  import DetailDrawer from '$lib/components/common/DetailDrawer.svelte';
  import { inferEdges, type CausalEdge } from '$lib/timeline/causality';

  let observations = $state<Observation[]>([]);
  let loading = $state(true);
  let error = $state('');
  let selectedId = $state<string | undefined>(undefined);
  let drawerObs = $state<Observation | null>(null);
  let listMode = $state(false);

  let sessionId = $derived(decodeURIComponent(page.params.id ?? ''));
  let timelineId = $derived(sessionId.replace(/^session:/, ''));

  onMount(async () => {
    try {
      const res = await getTimeline(timelineId, 500);
      observations = res.observations;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load';
    } finally {
      loading = false;
    }
  });

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
      start: new Date(first.timestamp),
      end: new Date(last.timestamp),
      durationLabel: mins < 60 ? `${mins}m` : `${Math.floor(mins / 60)}h ${mins % 60}m`,
    };
  });

  function onWaterfallSelect(obs: Observation | null) {
    drawerObs = obs;
    selectedId = obs ? extractId(obs.id) : undefined;
  }

  function onGraphSelect(n: GraphInputNode | null) {
    if (!n) {
      drawerObs = null;
      return;
    }
    drawerObs = n.raw as Observation;
    selectedId = n.id;
  }

  function fmt(d: Date): string {
    return d.toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  function isMuted(obs: Observation): boolean {
    return obs.title === 'unknown call' || obs.title === 'User submitted a prompt.';
  }

  function formatTime(ts: string): string {
    return new Date(ts).toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }
</script>

<div class="page" class:has-drawer={drawerObs !== null}>
  <header class="head">
    <h2 class="title">Session timeline</h2>
    {#if summary}
      <div class="meta">
        <span>{fmt(summary.start)} → {fmt(summary.end)}</span>
        <span class="dot">·</span>
        <span>{summary.durationLabel}</span>
        <span class="dot">·</span>
        <span>{summary.count} observations</span>
        <span class="dot">·</span>
        <span>{causalEdges.length} causal links</span>
      </div>
    {/if}
    <div class="mode-switch">
      <button class="mode-btn" class:active={!listMode} onclick={() => (listMode = false)}>Timeline</button>
      <button class="mode-btn" class:active={listMode} onclick={() => (listMode = true)}>List</button>
    </div>
  </header>

  {#if loading}
    <LoadingSpinner />
  {:else if error}
    <div class="card" style="border-color: var(--accent);">
      <p style="color: var(--accent); margin: 0;">{error}</p>
    </div>
  {:else if observations.length === 0}
    <p class="empty">No observations in this session.</p>
  {:else if !listMode}
    <section class="pane mini-graph">
      <h3 class="pane-h">Causal graph</h3>
      <div class="mini-graph-area">
        <CytoscapeGraph
          nodes={graphNodes}
          edges={graphEdges}
          bind:selectedId
          onSelect={onGraphSelect}
          compact={true}
        />
      </div>
    </section>

    <section class="pane waterfall-pane">
      <h3 class="pane-h">Time axis</h3>
      <Waterfall
        {observations}
        edges={causalEdges}
        bind:selectedId
        onSelect={onWaterfallSelect}
      />
    </section>
  {:else}
    <ol class="list">
      {#each observations as obs}
        {@const muted = isMuted(obs)}
        <li class="list-item" class:muted>
          <span class="list-time">{formatTime(obs.timestamp)}</span>
          <span class="badge badge-blue">{obs.obs_type}</span>
          <button
            type="button"
            class="list-title"
            onclick={() => onWaterfallSelect(obs)}
          >
            {obs.title}
          </button>
          {#if obs.importance >= 7}
            <span class="list-imp">★ {obs.importance}</span>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</div>

<DetailDrawer
  item={drawerObs ? { kind: 'observation', data: drawerObs } : null}
  onClose={() => {
    drawerObs = null;
    selectedId = undefined;
  }}
/>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 16px;
    flex-wrap: wrap;
  }
  .title {
    margin: 0;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 700;
  }
  .meta {
    display: flex;
    gap: 6px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-muted);
    flex-wrap: wrap;
  }
  .dot { color: var(--ink-faint); }

  .mode-switch {
    margin-left: auto;
    display: inline-flex;
    border: 1px solid var(--border);
  }
  .mode-btn {
    padding: 4px 12px;
    background: var(--bg);
    border: none;
    font-size: 10px;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-muted);
    cursor: pointer;
  }
  .mode-btn:not(:last-child) { border-right: 1px solid var(--border); }
  .mode-btn.active {
    background: var(--ink);
    color: var(--bg);
  }

  .pane {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .pane-h {
    margin: 0;
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-faint);
  }

  .mini-graph .mini-graph-area {
    height: 240px;
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

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .list-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border: 1px solid var(--border-light);
  }
  .list-item.muted { opacity: 0.5; }
  .list-time {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-muted);
    min-width: 70px;
  }
  .list-title {
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    flex: 1;
    font-family: var(--font-body);
    font-size: 12px;
    color: var(--ink);
    padding: 0;
  }
  .list-title:hover { color: var(--accent); }
  .list-imp {
    font-size: 11px;
    color: var(--yellow);
    font-family: var(--font-ui);
  }

  .has-drawer { padding-right: min(440px, 90vw); }
</style>
