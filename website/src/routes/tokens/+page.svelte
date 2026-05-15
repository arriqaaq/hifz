<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchProjectUsage, getSessions, type ProjectUsageView } from '$lib/api';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import StatCards from './_components/StatCards.svelte';
  import DailyChart from './_components/DailyChart.svelte';
  import ModelDonut from './_components/ModelDonut.svelte';
  import PatternsGrid from './_components/PatternsGrid.svelte';
  import TopPromptsTable from './_components/TopPromptsTable.svelte';
  import TopSessionsTable from './_components/TopSessionsTable.svelte';

  let projects = $state<string[]>([]);
  let project = $state<string>('');
  let from = $state<string>('');
  let to = $state<string>('');
  let view = $state<ProjectUsageView | null>(null);
  let loading = $state<boolean>(true);
  let error = $state<string>('');

  onMount(async () => {
    try {
      const res = await getSessions(200);
      const set = new Set<string>();
      for (const s of res.sessions ?? []) {
        if (s?.project) set.add(projectName(s.project));
      }
      projects = Array.from(set).sort();
      project = projects[0] ?? '';
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load projects';
      loading = false;
    }
  });

  async function load() {
    if (!project) {
      loading = false;
      return;
    }
    loading = true;
    error = '';
    try {
      view = await fetchProjectUsage(project, {
        from: from || undefined,
        to: to || undefined,
      });
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load usage';
      view = null;
    } finally {
      loading = false;
    }
  }

  function projectName(p: string): string {
    return p.split('/').pop() ?? p;
  }

  function setPreset(days: number) {
    const now = new Date();
    const past = new Date(now);
    past.setDate(now.getDate() - days);
    to = now.toISOString().slice(0, 10);
    from = past.toISOString().slice(0, 10);
    void load();
  }

  function clearRange() {
    from = '';
    to = '';
    void load();
  }
</script>

<div class="hero">
  <h1>Tokens</h1>
  <p class="lead">
    How many tokens this project's Claude Code sessions have consumed. Live as the
    Stop hook captures each turn.
  </p>
</div>

<div class="controls">
  <label class="control">
    <span class="control-label">Project</span>
    <select bind:value={project} onchange={load}>
      {#each projects as p}
        <option value={p}>{p}</option>
      {/each}
    </select>
  </label>
  <label class="control">
    <span class="control-label">From</span>
    <input type="date" bind:value={from} onchange={load} />
  </label>
  <label class="control">
    <span class="control-label">To</span>
    <input type="date" bind:value={to} onchange={load} />
  </label>
  <div class="presets">
    <button type="button" onclick={() => setPreset(7)}>7d</button>
    <button type="button" onclick={() => setPreset(30)}>30d</button>
    <button type="button" onclick={clearRange}>all</button>
  </div>
</div>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="error">{error}</div>
{:else if !view || view.call_count === 0}
  <div class="empty">
    <h2>No token data yet</h2>
    <p>
      Run the backfill script once to import history for this project, or just
      start a new Claude Code session — the Stop hook will populate this view
      after each turn.
    </p>
    <pre><code>node adapters/claude-code/scripts/backfill-tokens.mjs</code></pre>
  </div>
{:else if view}
  <StatCards totals={view.totals} callCount={view.call_count} sessionCount={view.session_count} dateRange={view.date_range} />
  <div class="charts">
    <div class="card">
      <h3>Daily tokens</h3>
      <DailyChart daily={view.daily} />
    </div>
    <div class="card">
      <h3>Models</h3>
      <ModelDonut models={view.models} />
    </div>
  </div>
  <PatternsGrid patterns={view.patterns} />
  <div class="card">
    <h3>Top 20 most expensive prompts</h3>
    <TopPromptsTable rows={view.top_prompts} />
  </div>
  <div class="card">
    <h3>Top sessions by tokens</h3>
    <TopSessionsTable rows={view.top_sessions} />
  </div>
{/if}

<style>
  .hero {
    margin-bottom: 18px;
  }
  .hero h1 {
    font-size: 24px;
    margin: 0 0 4px;
  }
  .lead {
    margin: 0;
    color: var(--ink-muted);
    font-size: 13px;
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: flex-end;
    margin-bottom: 18px;
    padding: 12px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
  }
  .control {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }
  .control-label {
    font-size: 10px;
    text-transform: uppercase;
    color: var(--ink-faint);
    letter-spacing: 0.04em;
  }
  .control select,
  .control input {
    background: var(--bg);
    border: 1px solid var(--line);
    color: var(--ink);
    padding: 4px 8px;
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .presets {
    display: flex;
    gap: 6px;
  }
  .presets button {
    background: var(--surface);
    border: 1px solid var(--line);
    color: var(--ink-muted);
    padding: 4px 10px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .presets button:hover {
    border-color: var(--line-strong);
    color: var(--ink);
  }
  .charts {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(0, 1fr);
    gap: 12px;
    margin-bottom: 18px;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    padding: 14px;
    margin-bottom: 14px;
  }
  .card h3 {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-faint);
    margin: 0 0 10px;
  }
  .error {
    padding: 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--c-bug, #c4453d);
  }
  .empty {
    padding: 24px;
    border: 1px dashed var(--line);
    border-radius: var(--radius-sm);
    text-align: center;
    color: var(--ink-muted);
  }
  .empty h2 {
    font-size: 14px;
    margin: 0 0 6px;
    color: var(--ink);
  }
  .empty pre {
    margin-top: 12px;
    background: var(--bg);
    padding: 10px;
    border-radius: var(--radius-sm);
    overflow-x: auto;
    font-size: 11px;
  }
</style>
