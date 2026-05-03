<script lang="ts">
  import { onMount } from 'svelte';
  import { getCommits } from '$lib/api';
  import type { Commit } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import EntityChip from '$lib/components/entity/EntityChip.svelte';
  import { extractId } from '$lib/components/entity/entityHelpers';

  let commits = $state<Commit[]>([]);
  let loading = $state(true);
  let error = $state('');
  let query = $state('');

  onMount(async () => {
    try {
      const r = await getCommits(undefined, 200);
      commits = r.commits;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load commits';
    } finally {
      loading = false;
    }
  });

  let filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commits;
    return commits.filter((c) => {
      const sha = c.metadata?.sha ?? '';
      const msg = c.metadata?.message ?? c.title;
      const branch = c.metadata?.branch ?? '';
      return (
        sha.toLowerCase().includes(q) ||
        msg.toLowerCase().includes(q) ||
        branch.toLowerCase().includes(q)
      );
    });
  });

  function fmtDate(ts: string): string {
    return new Date(ts).toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  function sessionIdOf(c: Commit): string | null {
    if (!c.session_id) return null;
    return extractId(c.session_id);
  }
</script>

<div class="page">
  <header class="head">
    <h2 class="title">Commits</h2>
    <input
      type="text"
      bind:value={query}
      placeholder="Filter by sha, message, or branch…"
      class="search-input"
    />
  </header>

  {#if loading}
    <LoadingSpinner />
  {:else if error}
    <div class="card" style="border-color: var(--danger);">
      <p style="color: var(--danger); margin: 0;">{error}</p>
    </div>
  {:else if filtered.length === 0}
    <p class="empty">{query ? 'No commits match your filter' : 'No commits recorded yet.'}</p>
  {:else}
    <p class="result-meta">{filtered.length} commits</p>
    <div class="card">
      <table>
        <thead>
          <tr>
            <th>SHA</th>
            <th>Message</th>
            <th>Branch</th>
            <th>Files</th>
            <th>Session</th>
            <th>Date</th>
          </tr>
        </thead>
        <tbody>
          {#each filtered as c}
            {@const sha = c.metadata?.sha ?? ''}
            {@const msg = c.metadata?.message ?? c.title}
            {@const branch = c.metadata?.branch ?? ''}
            {@const files = c.metadata?.files ?? c.files ?? []}
            {@const sid = sessionIdOf(c)}
            <tr>
              <td><EntityChip kind="commit" id={sha} size="sm" /></td>
              <td class="msg-cell">{msg}</td>
              <td>
                {#if branch}<span class="badge badge-blue">{branch}</span>{/if}
              </td>
              <td class="mono">{files.length}</td>
              <td>
                {#if sid}<EntityChip kind="session" id={sid} size="sm" />{/if}
              </td>
              <td class="mono faint">{fmtDate(c.timestamp)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
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
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }

  .title {
    margin: 0;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .search-input {
    flex: 1;
    min-width: 240px;
    max-width: 480px;
    padding: 8px 12px;
    font-size: 13px;
  }

  .result-meta {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-faint);
    margin: 0 0 8px;
  }

  .msg-cell {
    max-width: 480px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .faint {
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
