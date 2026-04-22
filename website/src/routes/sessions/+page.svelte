<script lang="ts">
  import { onMount } from 'svelte';
  import { getSessions } from '$lib/api';
  import type { Session } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let allSessions = $state<Session[]>([]);
  let loading = $state(true);
  let error = $state('');
  let searchQuery = $state('');

  let filteredSessions = $derived(() => {
    if (!searchQuery.trim()) return allSessions;
    const q = searchQuery.toLowerCase();
    return allSessions.filter(s => 
      s.project.toLowerCase().includes(q) ||
      (s.name && s.name.toLowerCase().includes(q)) ||
      s.id.toLowerCase().includes(q)
    );
  });

  onMount(async () => {
    try {
      const res = await getSessions(100);
      allSessions = res.sessions;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load';
    } finally {
      loading = false;
    }
  });

  function formatTime(ts: string): string {
    return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  function formatDate(ts: string): string {
    return new Date(ts).toLocaleDateString([], { month: 'short', day: 'numeric' });
  }

  function duration(start: string, end: string | null): string {
    if (!end) return 'active';
    const ms = new Date(end).getTime() - new Date(start).getTime();
    const mins = Math.floor(ms / 60000);
    if (mins < 60) return `${mins}m`;
    return `${Math.floor(mins / 60)}h ${mins % 60}m`;
  }

  function projectName(path: string): string {
    return path.split('/').pop() ?? path;
  }

  function extractId(id: unknown): string {
    if (typeof id === 'string') {
      return id.replace(/^session:/, '');
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
</script>

<div class="search-form">
  <div class="search-row">
    <input 
      type="text" 
      placeholder="Filter sessions by project or name..." 
      bind:value={searchQuery} 
      class="search-input" 
    />
  </div>
</div>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="card" style="border-color: var(--accent);">
    <p style="color: var(--accent); margin: 0;">{error}</p>
  </div>
{:else if filteredSessions().length === 0}
  <p class="empty">{searchQuery ? 'No sessions match your filter' : 'No sessions found'}</p>
{:else}
  <p class="result-meta">{filteredSessions().length} sessions</p>
  <div class="card">
    <table>
      <thead>
        <tr>
          <th>Project</th>
          <th>Name</th>
          <th>Status</th>
          <th>Observations</th>
          <th>Duration</th>
          <th>Date</th>
        </tr>
      </thead>
      <tbody>
        {#each filteredSessions() as s (extractId(s.id))}
          <tr>
            <td><a href="/sessions/{extractId(s.id)}" class="row-link">{projectName(s.project)}</a></td>
            <td class="name-cell">{s.name ?? '—'}</td>
            <td>
              {#if s.ended_at}
                <span class="badge badge-blue">completed</span>
              {:else}
                <span class="badge badge-green">active</span>
              {/if}
            </td>
            <td class="mono">{s.observation_count}</td>
            <td class="mono">{duration(s.started_at, s.ended_at)}</td>
            <td class="mono faint">{formatDate(s.started_at)} {formatTime(s.started_at)}</td>
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

  .row-link {
    font-weight: 700;
    color: var(--ink);
  }
  .row-link:hover { color: var(--accent); }

  .name-cell {
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--ink-muted);
  }

  .mono { font-family: var(--font-mono); font-size: 12px; }
  .faint { color: var(--ink-faint); }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }
</style>
