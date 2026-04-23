<script lang="ts">
  import { onMount } from 'svelte';
  import { searchObservations } from '$lib/api';
  import type { Observation } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let observations = $state<Observation[]>([]);
  let loading = $state(true);
  let query = $state('');
  let error = $state('');
  let expandedId = $state<string | null>(null);

  async function doSearch() {
    loading = true;
    error = '';
    try {
      const res = await searchObservations(query || '*', 100);
      observations = res.observations;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Search failed';
    } finally {
      loading = false;
    }
  }

  onMount(doSearch);

  function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    doSearch();
  }

  function formatTime(ts: string): string {
    return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  function formatDate(ts: string): string {
    return new Date(ts).toLocaleDateString([], { month: 'short', day: 'numeric' });
  }

  function toggleExpand(id: string) {
    expandedId = expandedId === id ? null : id;
  }

  function extractId(id: unknown): string {
    if (typeof id === 'string') return id;
    if (id && typeof id === 'object' && 'key' in id) {
      const key = (id as { key: unknown }).key;
      if (typeof key === 'string') return key;
      if (key && typeof key === 'object' && 'String' in key) {
        return (key as { String: string }).String;
      }
    }
    return String(id);
  }
</script>

<div class="search-form">
  <form onsubmit={handleSubmit} class="search-row">
    <input type="text" placeholder="Search observations..." bind:value={query} class="search-input" />
    <button type="submit" class="btn btn--accent btn--small">Search</button>
  </form>
</div>

{#if error}
  <div class="card" style="border-color: var(--accent);">
    <p style="color: var(--accent); margin: 0;">{error}</p>
  </div>
{/if}

{#if loading}
  <LoadingSpinner />
{:else if observations.length === 0}
  <p class="empty">No observations found. Observations are recorded when the agent runs tool calls.</p>
{:else}
  <p class="result-meta">{observations.length} observations</p>
  <div class="timeline">
    {#each observations as obs (extractId(obs.id))}
      {@const obsId = extractId(obs.id)}
      {@const isExpanded = expandedId === obsId}
      <div class="card obs-card" class:expanded={isExpanded}>
        <button class="obs-header" onclick={() => toggleExpand(obsId)}>
          <span class="obs-time">{formatDate(obs.timestamp)} {formatTime(obs.timestamp)}</span>
          <span class="badge badge-blue">{obs.obs_type}</span>
          {#if obs.importance >= 7}
            <span class="obs-imp">&#9733; {obs.importance}</span>
          {/if}
          <span class="expand-icon">{isExpanded ? '−' : '+'}</span>
        </button>
        <h4 class="obs-title">{obs.title}</h4>
        {#if obs.subtitle}
          <p class="obs-sub">{obs.subtitle}</p>
        {/if}
        {#if !isExpanded && obs.narrative}
          <p class="obs-narrative truncated">{obs.narrative.slice(0, 150)}{obs.narrative.length > 150 ? '...' : ''}</p>
        {/if}
        {#if isExpanded}
          {#if obs.narrative}
            {#if obs.obs_type === 'command_run' || obs.obs_type === 'file_edit' || obs.obs_type === 'file_write'}
              <pre class="obs-code"><code>{obs.narrative}</code></pre>
            {:else}
              <p class="obs-narrative">{obs.narrative}</p>
            {/if}
          {/if}
          {#if obs.facts && obs.facts.length > 0}
            <div class="obs-section">
              <span class="section-label">Facts</span>
              <ul class="obs-facts">
                {#each obs.facts as fact}
                  <li><code class="fact-code">{fact}</code></li>
                {/each}
              </ul>
            </div>
          {/if}
          {#if (obs.keywords && obs.keywords.length > 0) || (obs.files && obs.files.length > 0)}
            <div class="obs-tags">
              {#each obs.keywords || [] as c}
                <span class="badge badge-yellow">{c}</span>
              {/each}
              {#each obs.files || [] as f}
                <span class="badge badge-cyan" style="font-family: var(--font-mono); font-size: 9px">{f.split('/').pop()}</span>
              {/each}
            </div>
          {/if}
          <div class="obs-meta">
            <span>Importance: {obs.importance}</span>
            {#if obs.confidence}
              <span>Confidence: {obs.confidence}</span>
            {/if}
          </div>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .search-form { margin-bottom: 20px; }
  .search-row {
    display: flex;
    gap: 0;
    border: 1px solid var(--border);
  }
  .search-input {
    flex: 1;
    border: none;
    padding: 12px 16px;
    font-size: 14px;
    font-family: var(--font-body);
  }
  .search-input:focus { border: none; }

  .result-meta {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-faint);
    margin-bottom: 12px;
  }

  .timeline {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 900px;
  }

  .obs-card {
    padding: 12px 16px;
    cursor: pointer;
    transition: border-color 150ms;
  }
  .obs-card:hover {
    border-color: var(--ink-muted);
  }
  .obs-card.expanded {
    border-color: var(--accent);
  }

  .obs-header {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 6px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
  }

  .obs-time {
    font-family: var(--font-mono);
    font-size: 11px;
    font-weight: 500;
    color: var(--ink-muted);
  }

  .obs-imp {
    font-size: 11px;
    color: var(--yellow);
  }

  .expand-icon {
    margin-left: auto;
    font-size: 16px;
    font-weight: bold;
    color: var(--ink-faint);
  }

  .obs-title {
    margin: 0 0 4px;
    font-size: 13px;
    font-weight: 700;
    font-family: var(--font-display);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .obs-sub {
    margin: 0 0 6px;
    font-size: 11px;
    color: var(--ink-muted);
    font-family: var(--font-ui);
  }

  .obs-narrative {
    margin: 0 0 10px;
    font-size: 12px;
    color: var(--ink-secondary);
    line-height: 1.5;
  }
  .obs-narrative.truncated {
    color: var(--ink-faint);
    font-style: italic;
  }

  .obs-code {
    margin: 0 0 10px;
    padding: 10px 14px;
    background: var(--bg-alt, #F0F0EC);
    border: 1px solid var(--border-light);
    font-family: var(--font-mono);
    font-size: 10px;
    line-height: 1.5;
    color: var(--ink-secondary);
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 300px;
  }
  .obs-code code {
    font-family: inherit;
    font-size: inherit;
  }

  .obs-section {
    margin-bottom: 10px;
  }
  .section-label {
    display: block;
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-faint);
    margin-bottom: 4px;
  }

  .obs-facts {
    margin: 0;
    padding-left: 16px;
    font-size: 11px;
    color: var(--ink-secondary);
    line-height: 1.5;
  }
  .obs-facts li { margin-bottom: 2px; }

  .fact-code {
    font-family: var(--font-mono);
    font-size: 10px;
    background: var(--bg-alt, #F0F0EC);
    padding: 1px 4px;
    border-radius: 2px;
  }

  .obs-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-bottom: 8px;
  }

  .obs-meta {
    display: flex;
    gap: 16px;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
  }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }
</style>
