<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getAtlasInsights,
    atlasQuery,
    type AtlasInsights,
    type AtlasHit,
  } from '$lib/api';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let ins = $state<AtlasInsights | null>(null);
  let loading = $state(true);
  let error = $state('');

  let q = $state('');
  let hits = $state<AtlasHit[]>([]);
  let searching = $state(false);

  onMount(async () => {
    try {
      ins = await getAtlasInsights();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  });

  async function runQuery() {
    if (!q.trim()) return;
    searching = true;
    try {
      const r = await atlasQuery(q.trim());
      hits = r.hits ?? [];
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      searching = false;
    }
  }
</script>

<div class="page">
  <header class="head">
    <h1>Atlas</h1>
    <p class="sub">Corpus knowledge graph — documents, concepts, and the code graph, clustered.</p>
  </header>

  {#if loading}
    <LoadingSpinner />
  {:else if error}
    <div class="card"><span class="badge badge-red">error</span> {error}</div>
  {:else if ins}
    <div class="stats">
      <div class="stat-card">
        <div class="label">Nodes</div>
        <div class="value">{ins.nodes}</div>
      </div>
      <div class="stat-card">
        <div class="label">Edges</div>
        <div class="value">{ins.edges}</div>
      </div>
      <div class="stat-card">
        <div class="label">Clusters</div>
        <div class="value">{ins.clusters}</div>
      </div>
    </div>

    <div class="card">
      <div class="card-title">Query</div>
      <div class="qrow">
        <input
          placeholder="search the corpus graph…"
          bind:value={q}
          onkeydown={(e) => e.key === 'Enter' && runQuery()}
        />
        <button class="btn btn--accent" onclick={runQuery} disabled={searching}>
          {searching ? '…' : 'Search'}
        </button>
      </div>
      {#if hits.length}
        <table>
          <thead><tr><th>Kind</th><th>Label</th><th>Snippet</th></tr></thead>
          <tbody>
            {#each hits as h}
              <tr>
                <td><span class="badge badge-blue">{h.kind}</span></td>
                <td class="mono">{h.label}</td>
                <td>{h.snippet ?? ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>

    <div class="card">
      <div class="card-title">Hub nodes — most connected</div>
      <table>
        <thead><tr><th>Label</th><th>Kind</th><th>Weighted degree</th><th>Clusters touched</th></tr></thead>
        <tbody>
          {#each ins.hub_nodes as h}
            <tr>
              <td class="mono">{h.label}</td>
              <td><span class="badge badge-purple">{h.kind}</span></td>
              <td>{h.weighted_degree.toFixed(2)}</td>
              <td>{h.clusters_touched}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="card">
      <div class="card-title">Surprising cross-cluster links</div>
      <table>
        <thead><tr><th>From</th><th>Relation</th><th>To</th><th>Why</th></tr></thead>
        <tbody>
          {#each ins.surprising_links as s}
            <tr>
              <td class="mono">{s.from}</td>
              <td><span class="badge badge-orange">{s.relation}</span></td>
              <td class="mono">{s.to}</td>
              <td>{s.why}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="card">
      <div class="card-title">Isolated nodes — gaps</div>
      <table>
        <thead><tr><th>Label</th><th>Kind</th><th>Degree</th></tr></thead>
        <tbody>
          {#each ins.isolated_nodes as i}
            <tr>
              <td class="mono">{i.label}</td>
              <td><span class="badge badge-gray">{i.kind}</span></td>
              <td>{i.weighted_degree.toFixed(2)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .page { padding: 24px; max-width: 1100px; margin: 0 auto; }
  .head { margin-bottom: 18px; }
  .head h1 { margin: 0; font-size: 22px; font-weight: 700; }
  .head .sub { color: var(--ink-muted); font-size: 13px; margin: 4px 0 0; }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 14px; }
  .qrow { display: flex; gap: 8px; margin-bottom: 12px; }
  .qrow input { flex: 1; }
</style>
