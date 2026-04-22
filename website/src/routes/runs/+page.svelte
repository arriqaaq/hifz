<script lang="ts">
  import { onMount } from 'svelte';
  import { searchRuns } from '$lib/api';
  import type { Run } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let runs = $state<Run[]>([]);
  let loading = $state(true);
  let query = $state('');
  let error = $state('');

  async function doSearch() {
    loading = true;
    error = '';
    try {
      const res = await searchRuns(query || '*');
      runs = res.runs;
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

  function extractId(id: unknown): string {
    if (typeof id === 'string') {
      const parts = id.split(':');
      return parts.length > 1 ? parts[1] : id;
    }
    if (id && typeof id === 'object' && 'key' in id) {
      const key = (id as { key: unknown }).key;
      if (typeof key === 'string') return key;
      if (key && typeof key === 'object' && 'String' in key) {
        return (key as { String: string }).String;
      }
    }
    return String(id);
  }

  function extractSessionId(sessionId: unknown): string {
    if (typeof sessionId === 'string') {
      return sessionId.replace(/^session:/, '');
    }
    if (sessionId && typeof sessionId === 'object' && 'key' in sessionId) {
      const key = (sessionId as { key: unknown }).key;
      if (typeof key === 'string') return key;
      if (key && typeof key === 'object' && 'String' in key) {
        return (key as { String: string }).String;
      }
    }
    return String(sessionId);
  }

  function outcomeClass(outcome: string): string {
    switch (outcome) {
      case 'committed': return 'badge-green';
      case 'success': return 'badge-blue';
      case 'uncommitted': return 'badge-yellow';
      case 'failure': return 'badge-red';
      default: return '';
    }
  }
</script>

<div class="search-form">
  <form onsubmit={handleSubmit} class="search-row">
    <input type="text" placeholder="Search runs..." bind:value={query} class="search-input" />
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
{:else if runs.length === 0}
  <p class="empty">No runs recorded yet. Runs are created when the agent completes task trajectories.</p>
{:else}
  <p class="result-meta">{runs.length} runs</p>
  <div class="card">
    <table>
      <thead>
        <tr>
          <th>Outcome</th>
          <th>Prompt</th>
          <th>Lesson</th>
          <th>Obs</th>
          <th>Session</th>
          <th>Date</th>
        </tr>
      </thead>
      <tbody>
        {#each runs as run (extractId(run.id))}
          <tr class="run-row">
            <td>
              <span class="badge {outcomeClass(run.outcome)}">{run.outcome}</span>
            </td>
            <td class="prompt-cell">
              <a href="/runs/{extractId(run.id)}" class="prompt-link">{run.prompt}</a>
            </td>
            <td class="lesson-cell">{run.lesson ?? '—'}</td>
            <td class="mono">{run.observation_ids?.length ?? 0}</td>
            <td><a href="/sessions/{extractSessionId(run.session_id)}" class="row-link">view</a></td>
            <td class="mono faint">{formatDate(run.started_at)} {formatTime(run.started_at)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
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

  .run-row {
    transition: background-color 150ms;
  }
  .run-row:hover {
    background-color: var(--bg-alt, #F8F8F6);
  }

  .prompt-cell {
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .prompt-link {
    font-weight: 600;
    color: var(--ink);
    transition: color 150ms;
  }
  .prompt-link:hover {
    color: var(--accent);
  }

  .lesson-cell {
    max-width: 250px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-muted);
    font-size: 12px;
    font-style: italic;
  }
  .row-link {
    font-size: 11px;
    font-weight: 600;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-muted);
  }
  .row-link:hover { color: var(--accent); }
  .mono { font-family: var(--font-mono); font-size: 12px; }
  .faint { color: var(--ink-faint); }
  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }

  .badge-green {
    background: var(--green, #2ECC71);
    color: white;
  }
  .badge-red {
    background: var(--red, #E74C3C);
    color: white;
  }
</style>
