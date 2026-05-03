<script lang="ts">
  import { onMount } from 'svelte';
  import { searchMemories, forget } from '$lib/api';
  import type { Memory } from '$lib/types';
  import {
    CATEGORIES,
    categoryColor,
    categoryLabel,
    isLongForm,
  } from '$lib/ontology';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let memories = $state<Memory[]>([]);
  let loading = $state(true);
  let query = $state('');
  let error = $state('');
  let expandedId = $state<string | null>(null);

  // Phase 8.3: typed filters (category, project, time, open-only) and sorts.
  let filterCategory = $state<string>('');
  let filterProject = $state<string>('');
  let filterOpenOnly = $state(false);
  let filterSinceDays = $state<string>(''); // e.g. "30"
  let sortBy = $state<'strength' | 'recent' | 'access'>('strength');
  let groupByCategory = $state(false);

  async function doSearch() {
    loading = true;
    error = '';
    try {
      const sinceIso = filterSinceDays
        ? new Date(Date.now() - Number(filterSinceDays) * 86400_000).toISOString()
        : undefined;
      const res = await searchMemories(
        query || undefined,
        50,
        filterProject || undefined,
        filterCategory || undefined,
        { since: sinceIso, open: filterOpenOnly || undefined },
      );
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

  // Sort and optionally group the result set in-memory.
  let sortedMemories = $derived.by(() => {
    const list = [...memories];
    list.sort((a, b) => {
      switch (sortBy) {
        case 'recent':
          return (b.last_accessed_at ?? '').localeCompare(a.last_accessed_at ?? '');
        case 'access':
          return (b.retrieval_count ?? 0) - (a.retrieval_count ?? 0);
        case 'strength':
        default: {
          const sa = (a.strength ?? 1) * Math.log((a.retrieval_count ?? 0) + 2);
          const sb = (b.strength ?? 1) * Math.log((b.retrieval_count ?? 0) + 2);
          return sb - sa;
        }
      }
    });
    return list;
  });

  let groupedMemories = $derived.by(() => {
    if (!groupByCategory) return null;
    const groups = new Map<string, Memory[]>();
    for (const m of sortedMemories) {
      const k = m.category ?? 'note';
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k)!.push(m);
    }
    return groups;
  });
</script>

<div class="search-form">
  <form onsubmit={handleSubmit} class="search-row">
    <input type="text" placeholder="Search memories..." bind:value={query} class="search-input" />
    <button type="submit" class="btn btn--accent btn--small">Search</button>
  </form>
  <div class="filters">
    <select bind:value={filterCategory} onchange={doSearch} title="Filter by typed category">
      <option value="">All categories</option>
      {#each CATEGORIES as c}
        <option value={c}>{categoryLabel(c)}</option>
      {/each}
    </select>
    <input
      type="text"
      placeholder="Project"
      bind:value={filterProject}
      onchange={doSearch}
      class="filter-input"
    />
    <select bind:value={filterSinceDays} onchange={doSearch} title="Limit to recent activity">
      <option value="">Any time</option>
      <option value="1">Last 24h</option>
      <option value="7">Last 7d</option>
      <option value="30">Last 30d</option>
      <option value="90">Last 90d</option>
    </select>
    <label class="check-label" title="For Bug rows: hide ones closed by a Fix">
      <input type="checkbox" bind:checked={filterOpenOnly} onchange={doSearch} />
      Open only
    </label>
    <select bind:value={sortBy} title="Sort order">
      <option value="strength">Strength × access</option>
      <option value="recent">Most recent</option>
      <option value="access">Most accessed</option>
    </select>
    <label class="check-label">
      <input type="checkbox" bind:checked={groupByCategory} />
      Group by category
    </label>
  </div>
</div>

{#if error}
  <div class="card" style="border-color: var(--danger);">
    <p style="color: var(--danger); margin: 0;">{error}</p>
  </div>
{/if}

{#snippet memCard(mem: Memory)}
  {@const memId = extractId(mem.id)}
  {@const isExpanded = expandedId === memId}
  <div class="card mem-card" class:expanded={isExpanded} class:long-form={isLongForm(mem.category)}>
    <button class="mem-header" onclick={() => toggleExpand(memId)}>
      <span class="badge {categoryColor(mem.category)}">{categoryLabel(mem.category)}</span>
      {#if isLongForm(mem.category)}
        <span class="doc-icon" title="Long-form artifact (chunked for retrieval)">📄</span>
      {/if}
      <span class="mem-title">{mem.title}</span>
      <span class="mem-stats">
        <span title="Strength">● {(mem.strength ?? 1).toFixed(2)}</span>
        <span title="Retrieval count">👁 {mem.retrieval_count ?? 0}</span>
      </span>
      <a class="open-btn" href={`/memories/${encodeURIComponent(memId)}`} onclick={(e) => e.stopPropagation()} title="Open detail page">↗</a>
      <span class="expand-icon">{isExpanded ? '−' : '+'}</span>
    </button>

    {#if !isExpanded}
      <p class="mem-preview">
        {mem.context_summary || mem.content?.slice(0, 120) || ''}{!mem.context_summary && (mem.content?.length ?? 0) > 120 ? '…' : ''}
      </p>
    {/if}

    {#if isExpanded}
      <div class="mem-content">
        {#if mem.context_summary}
          <blockquote class="ctx-summary">{mem.context_summary}</blockquote>
        {/if}
        <pre class="content-text">{mem.content}</pre>
      </div>

      {#if (mem.keywords?.length || mem.tags?.length || mem.files?.length)}
        <div class="mem-tags">
          {#each mem.tags || [] as t}
            <span class="badge badge-purple" title="LLM-generated tag">#{t}</span>
          {/each}
          {#each mem.keywords || [] as k}
            <span class="badge badge-yellow">{k}</span>
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
        <a class="open-btn-text" href={`/memories/${encodeURIComponent(memId)}`}>Open ↗</a>
        <button class="del-btn" onclick={() => handleDelete(memId)} title="Delete memory">× Delete</button>
      </div>
    {/if}
  </div>
{/snippet}

{#if loading}
  <LoadingSpinner />
{:else if sortedMemories.length === 0}
  <p class="empty">No memories found. Use hifz_save to store lessons, decisions, plans, and more.</p>
{:else}
  <p class="result-meta">{sortedMemories.length} memories</p>
  <div class="memories-list">
    {#if groupedMemories}
      {#each [...groupedMemories.entries()] as [cat, group]}
        <h3 class="cat-header">
          <span class="badge {categoryColor(cat)}">{categoryLabel(cat)}</span>
          <span class="cat-count">{group.length}</span>
        </h3>
        {#each group as mem (extractId(mem.id))}
          {@render memCard(mem)}
        {/each}
      {/each}
    {:else}
      {#each sortedMemories as mem (extractId(mem.id))}
        {@render memCard(mem)}
      {/each}
    {/if}
  </div>
{/if}

<style>
  .search-form { margin-bottom: 20px; }
  .search-row {
    display: flex;
    gap: 0;
    border: 1px solid var(--line-strong);
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
    border-color: var(--ink);
    box-shadow: 4px 4px 0 var(--neon-dim);
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
    background: var(--surface-alt);
    border: 1px solid var(--line);
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
    border-top: 1px solid var(--line);
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
  .del-btn:hover { color: var(--ink); text-decoration: underline; text-decoration-color: var(--neon); text-decoration-thickness: 2px; text-underline-offset: 2px; }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }


  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-top: 8px;
    font-family: var(--font-ui);
    font-size: 11px;
  }
  .filters select,
  .filters .filter-input {
    border: 1px solid var(--line-strong);
    background: var(--surface);
    padding: 6px 8px;
    font-size: 11px;
    font-family: inherit;
  }
  .check-label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    cursor: pointer;
  }

  .doc-icon { font-size: 12px; }
  .open-btn {
    text-decoration: none;
    color: var(--ink-faint);
    font-size: 14px;
    padding: 0 6px;
  }
  .open-btn:hover { color: var(--ink); text-decoration: underline; text-decoration-color: var(--neon); text-decoration-thickness: 2px; text-underline-offset: 2px; }
  .open-btn-text {
    margin-left: auto;
    text-decoration: none;
    color: var(--ink-faint);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .open-btn-text:hover { color: var(--ink); text-decoration: underline; text-decoration-color: var(--neon); text-decoration-thickness: 2px; text-underline-offset: 2px; }

  .mem-card.long-form {
    border-left: 4px solid var(--neon);
  }

  .ctx-summary {
    margin: 0 0 8px 0;
    padding: 8px 12px;
    border-left: 3px solid var(--ink-muted);
    background: var(--surface-alt);
    color: var(--ink-muted);
    font-size: 12px;
    font-style: italic;
  }

  .cat-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 16px 0 4px;
    font-size: 12px;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .cat-count {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
  }
</style>
