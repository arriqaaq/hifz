<script lang="ts">
  import { onMount } from 'svelte';
  import { searchMemories, forget } from '$lib/api';
  import type { Hifz } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let memories = $state<Hifz[]>([]);
  let loading = $state(true);
  let query = $state('');
  let error = $state('');
  let expandedId = $state<string | null>(null);

  async function doSearch() {
    loading = true;
    error = '';
    try {
      const res = await searchMemories(query || undefined, 50);
      memories = res.memories;
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

  async function handleDelete(id: string) {
    try {
      await forget(id);
      memories = memories.filter(m => extractId(m.id) !== id);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Delete failed';
    }
  }

  function formatDate(ts: string): string {
    return new Date(ts).toLocaleDateString([], { month: 'short', day: 'numeric', year: 'numeric' });
  }

  function toggleExpand(id: string) {
    expandedId = expandedId === id ? null : id;
  }

  function typeColor(memType: string): string {
    switch (memType) {
      case 'pattern': return 'badge-purple';
      case 'preference': return 'badge-cyan';
      case 'architecture': return 'badge-blue';
      case 'bug': return 'badge-red';
      case 'workflow': return 'badge-yellow';
      default: return 'badge-green';
    }
  }
</script>

<div class="search-form">
  <form onsubmit={handleSubmit} class="search-row">
    <input type="text" placeholder="Search memories..." bind:value={query} class="search-input" />
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
{:else if memories.length === 0}
  <p class="empty">No memories found. Use hifz_save to store patterns, preferences, and knowledge.</p>
{:else}
  <p class="result-meta">{memories.length} memories</p>
  <div class="memories-list">
    {#each memories as mem (extractId(mem.id))}
      {@const memId = extractId(mem.id)}
      {@const isExpanded = expandedId === memId}
      <div class="card mem-card" class:expanded={isExpanded}>
        <button class="mem-header" onclick={() => toggleExpand(memId)}>
          <span class="badge {typeColor(mem.mem_type)}">{mem.mem_type}</span>
          <span class="mem-title">{mem.title}</span>
          <span class="mem-stats">
            <span title="Strength">&#9679; {(mem.strength ?? 1).toFixed(2)}</span>
            <span title="Access count">&#128065; {mem.access_count ?? 0}</span>
          </span>
          <span class="expand-icon">{isExpanded ? '−' : '+'}</span>
        </button>
        
        {#if !isExpanded}
          <p class="mem-preview">{mem.content?.slice(0, 120)}{(mem.content?.length ?? 0) > 120 ? '...' : ''}</p>
        {/if}

        {#if isExpanded}
          <div class="mem-content">
            <pre class="content-text">{mem.content}</pre>
          </div>

          {#if (mem.concepts && mem.concepts.length > 0) || (mem.files && mem.files.length > 0)}
            <div class="mem-tags">
              {#each mem.concepts || [] as c}
                <span class="badge badge-yellow">{c}</span>
              {/each}
              {#each mem.files || [] as f}
                <span class="badge badge-cyan" style="font-family: var(--font-mono); font-size: 9px">{f.split('/').pop()}</span>
              {/each}
            </div>
          {/if}

          <div class="mem-meta">
            <span>Created: {formatDate(mem.created_at)}</span>
            <span>Project: {mem.project}</span>
            {#if mem.version > 1}
              <span>Version: {mem.version}</span>
            {/if}
            <button class="del-btn" onclick={() => handleDelete(memId)} title="Delete memory">&times; Delete</button>
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

  .memories-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 900px;
  }

  .mem-card {
    padding: 12px 16px;
    cursor: pointer;
    transition: border-color 150ms;
  }
  .mem-card:hover {
    border-color: var(--ink-muted);
  }
  .mem-card.expanded {
    border-color: var(--accent);
    cursor: default;
  }

  .mem-header {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    margin-bottom: 6px;
  }

  .mem-title {
    flex: 1;
    font-weight: 700;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mem-stats {
    display: flex;
    gap: 12px;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
  }

  .expand-icon {
    font-size: 16px;
    font-weight: bold;
    color: var(--ink-faint);
    width: 20px;
    text-align: center;
  }

  .mem-preview {
    margin: 0;
    font-size: 12px;
    color: var(--ink-muted);
    font-style: italic;
  }

  .mem-content {
    margin: 10px 0;
  }

  .content-text {
    margin: 0;
    padding: 12px;
    background: var(--bg-alt, #F0F0EC);
    border: 1px solid var(--border-light);
    font-family: var(--font-body);
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 300px;
    overflow-y: auto;
  }

  .mem-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-bottom: 10px;
  }

  .mem-meta {
    display: flex;
    gap: 16px;
    align-items: center;
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
    padding-top: 8px;
    border-top: 1px solid var(--border-light);
  }

  .del-btn {
    margin-left: auto;
    font-size: 11px;
    font-weight: 600;
    font-family: var(--font-ui);
    color: var(--ink-faint);
    padding: 4px 8px;
    transition: color 150ms;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .del-btn:hover { color: var(--accent); }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }

  .badge-purple {
    background: #9B59B6;
    color: white;
  }
  .badge-red {
    background: #E74C3C;
    color: white;
  }
</style>
