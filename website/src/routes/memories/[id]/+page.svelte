<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { marked } from 'marked';
  import {
    searchMemories,
    getMemoryNeighbors,
    getMemoryBacklinks,
    getMemoryMarkdown,
    putMemoryMarkdown,
    forget,
  } from '$lib/api';
  import {
    categoryColor,
    categoryLabel,
    isLongForm,
    relationGroup,
    relationGroupColor,
    relationGroupLabel,
    type RelationGroup,
  } from '$lib/ontology';
  import type { Memory, MemoryEdge } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let memId = $derived($page.params.id ?? '');
  let memory = $state<Memory | null>(null);
  let outgoing = $state<MemoryEdge[]>([]);
  let backlinks = $state<MemoryEdge[]>([]);
  let loading = $state(true);
  let error = $state('');

  // Edit mode
  let editMode = $state(false);
  let editBuffer = $state('');
  let saving = $state(false);

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

  async function load() {
    loading = true;
    error = '';
    try {
      // Fetch the memory by id (filter the search endpoint).
      const id = memId;
      const res = await searchMemories(undefined, 1000);
      memory =
        res.memories.find((m) => extractId(m.id) === id || `memory:${extractId(m.id)}` === id) ??
        null;
      if (!memory) {
        error = 'Memory not found';
        return;
      }

      const [neigh, back] = await Promise.all([
        getMemoryNeighbors(id, { maxHops: 1 }).catch(() => ({ neighbors: [], count: 0 })),
        getMemoryBacklinks(id).catch(() => ({ backlinks: [], count: 0 })),
      ]);
      outgoing = neigh.neighbors;
      backlinks = back.backlinks;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Load failed';
    } finally {
      loading = false;
    }
  }

  onMount(load);

  // Re-render markdown body. Long-form rows: prefer `content_long`. Short:
  // `content`. Wikilink-style `[[id-or-title]]` references resolve to internal
  // routes via a post-render DOM walk.
  let renderedBody = $derived.by(() => {
    if (!memory) return '';
    const body = memory.content_long || memory.content || '';
    let html = marked.parse(body, { async: false }) as string;
    // Resolve `[[memory:abc]]` and `[[some title]]` references.
    html = html.replace(/\[\[([^\]]+)\]\]/g, (_, target: string) => {
      const slug = encodeURIComponent(target.trim());
      return `<a class="wikilink" href="/memories/${slug}">${target}</a>`;
    });
    return html;
  });

  // Group outgoing edges by relation group for the right panel.
  let outgoingByGroup = $derived.by(() => {
    const groups = new Map<RelationGroup | 'other', MemoryEdge[]>();
    for (const e of outgoing) {
      const g = relationGroup(e.relation);
      if (!groups.has(g)) groups.set(g, []);
      groups.get(g)!.push(e);
    }
    return groups;
  });

  let backlinksByRelation = $derived.by(() => {
    const groups = new Map<string, MemoryEdge[]>();
    for (const e of backlinks) {
      const r = e.relation || 'other';
      if (!groups.has(r)) groups.set(r, []);
      groups.get(r)!.push(e);
    }
    return groups;
  });

  async function startEdit() {
    if (!memory) return;
    saving = true;
    try {
      editBuffer = await getMemoryMarkdown(extractId(memory.id));
      editMode = true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not load markdown';
    } finally {
      saving = false;
    }
  }

  async function saveEdit() {
    if (!memory) return;
    if (
      !confirm(
        'Saving will write a NEW memory version that supersedes this one. The old version is preserved (is_latest=false). Continue?',
      )
    )
      return;
    saving = true;
    try {
      const r = await putMemoryMarkdown(extractId(memory.id), editBuffer);
      // Navigate to the new id so the URL is canonical.
      window.location.href = `/memories/${encodeURIComponent(r.id)}`;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Save failed';
      saving = false;
    }
  }

  async function handleDelete() {
    if (!memory) return;
    if (!confirm('Delete this memory? (sets is_latest=false; not a hard delete)')) return;
    try {
      await forget(extractId(memory.id));
      window.location.href = '/memories';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Delete failed';
    }
  }

  function formatDate(ts: string | undefined): string {
    if (!ts) return '';
    return new Date(ts).toLocaleString([], {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }
</script>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="card error">{error}</div>
{:else if memory}
  <div class="detail-grid">
    <!-- LEFT: metadata -->
    <aside class="left">
      <div class="card">
        <h3>Metadata</h3>
        <dl>
          <dt>Category</dt>
          <dd><span class="badge {categoryColor(memory.category)}">{categoryLabel(memory.category)}</span></dd>
          <dt>Project</dt>
          <dd>{memory.project}</dd>
          <dt>Created</dt>
          <dd>{formatDate(memory.created_at)}</dd>
          <dt>Updated</dt>
          <dd>{formatDate(memory.updated_at)}</dd>
          <dt>Version</dt>
          <dd>{memory.version}</dd>
          <dt>Strength</dt>
          <dd>{(memory.strength ?? 1).toFixed(2)}</dd>
          <dt>Recalls</dt>
          <dd>{memory.retrieval_count ?? 0}</dd>
          {#if memory.is_latest === false}
            <dt>Status</dt>
            <dd class="superseded">superseded</dd>
          {/if}
        </dl>
      </div>

      {#if memory.context_summary}
        <div class="card">
          <h3>Context summary</h3>
          <blockquote>{memory.context_summary}</blockquote>
        </div>
      {/if}

      {#if memory.keywords?.length}
        <div class="card">
          <h3>Keywords <span class="hint">(caller)</span></h3>
          <div class="chips">
            {#each memory.keywords as k}
              <span class="badge badge-yellow">{k}</span>
            {/each}
          </div>
        </div>
      {/if}

      {#if memory.tags?.length}
        <div class="card">
          <h3>Tags <span class="hint">(LLM)</span></h3>
          <div class="chips">
            {#each memory.tags as t}
              <span class="badge badge-purple">#{t}</span>
            {/each}
          </div>
        </div>
      {/if}

      {#if memory.files?.length}
        <div class="card">
          <h3>Files</h3>
          <ul class="files">
            {#each memory.files as f}
              <li><code>{f}</code></li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if memory.evolution_history?.length}
        <div class="card">
          <h3>Evolution history <span class="hint">({memory.evolution_history.length})</span></h3>
          <ol class="evo">
            {#each memory.evolution_history as e}
              <li>
                <div class="evo-head">
                  <span class="evo-field">{e.field}</span>
                  <span class="evo-time">{formatDate(e.timestamp)}</span>
                </div>
                <div class="evo-reason">{e.reason}</div>
                {#if e.previous}
                  <details><summary>previous</summary><pre>{e.previous}</pre></details>
                {/if}
              </li>
            {/each}
          </ol>
        </div>
      {/if}
    </aside>

    <!-- CENTER: rendered body -->
    <main class="center">
      <div class="title-row">
        <h1>{memory.title}</h1>
        <div class="actions">
          {#if !editMode}
            <button class="btn" onclick={startEdit} disabled={saving}>
              {isLongForm(memory.category) ? 'Edit markdown' : 'Edit'}
            </button>
            <button class="btn danger" onclick={handleDelete}>Delete</button>
          {:else}
            <button class="btn primary" onclick={saveEdit} disabled={saving}>
              {saving ? 'Saving…' : 'Save (supersedes)'}
            </button>
            <button class="btn" onclick={() => (editMode = false)}>Cancel</button>
          {/if}
        </div>
      </div>

      {#if editMode}
        <textarea class="edit-area" bind:value={editBuffer} spellcheck="false"></textarea>
      {:else}
        <article class="md-body">{@html renderedBody}</article>
      {/if}
    </main>

    <!-- RIGHT: relations -->
    <aside class="right">
      <div class="card">
        <h3>Outgoing edges <span class="hint">({outgoing.length})</span></h3>
        {#if outgoing.length === 0}
          <p class="empty">No outgoing edges.</p>
        {:else}
          {#each [...outgoingByGroup.entries()] as [group, edges]}
            <div class="rel-group">
              <h4 style="color: {relationGroupColor(group)}">{relationGroupLabel(group)}</h4>
              <ul class="rel-list">
                {#each edges as e}
                  <li>
                    <a href={e.id ? `/memories/${encodeURIComponent(e.id)}` : '#'}>
                      <span class="rel-name" style="color: {relationGroupColor(group)}">{e.relation}</span>
                      <span class="rel-target">{e.title || e.id}</span>
                    </a>
                    <div class="rel-meta">
                      <span title="Score">{(e.score ?? 0).toFixed(2)}</span>
                      <span class="via">via {e.via}</span>
                    </div>
                    {#if e.reason}
                      <div class="rel-reason">{e.reason}</div>
                    {/if}
                  </li>
                {/each}
              </ul>
            </div>
          {/each}
        {/if}
      </div>

      <div class="card">
        <h3>Backlinks <span class="hint">({backlinks.length})</span></h3>
        {#if backlinks.length === 0}
          <p class="empty">Nothing references this memory yet.</p>
        {:else}
          {#each [...backlinksByRelation.entries()] as [rel, edges]}
            <div class="rel-group">
              <h4>{rel}</h4>
              <ul class="rel-list">
                {#each edges as e}
                  <li>
                    <a href={e.id ? `/memories/${encodeURIComponent(e.id)}` : '#'}>
                      <span class="rel-target">{e.title || e.id}</span>
                    </a>
                    <div class="rel-meta">
                      <span title="Score">{(e.score ?? 0).toFixed(2)}</span>
                      <span class="via">via {e.via}</span>
                    </div>
                    {#if e.reason}
                      <div class="rel-reason">{e.reason}</div>
                    {/if}
                  </li>
                {/each}
              </ul>
            </div>
          {/each}
        {/if}
      </div>
    </aside>
  </div>
{/if}

<style>
  .detail-grid {
    display: grid;
    grid-template-columns: 280px 1fr 320px;
    gap: 16px;
    align-items: start;
  }
  @media (max-width: 1100px) {
    .detail-grid { grid-template-columns: 1fr; }
  }

  .left .card,
  .right .card {
    margin-bottom: 12px;
    padding: 12px;
  }
  .left h3,
  .right h3 {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-faint);
    margin: 0 0 8px;
  }
  .hint {
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0;
    color: var(--ink-faint);
    margin-left: 4px;
  }

  dl { display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; margin: 0; }
  dt { font-family: var(--font-mono); font-size: 10px; color: var(--ink-faint); }
  dd { margin: 0; font-size: 12px; }
  .superseded { color: var(--c-obs); font-weight: 600; }

  blockquote { margin: 0; padding: 8px 12px; border-left: 3px solid var(--ink-muted); background: var(--surface-alt); font-size: 12px; font-style: italic; }

  .chips { display: flex; flex-wrap: wrap; gap: 4px; }
  .files { margin: 0; padding-left: 16px; font-size: 12px; }
  .files li code { font-family: var(--font-mono); }

  .center { min-width: 0; }
  .title-row { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; }
  .title-row h1 { flex: 1; margin: 0; font-size: 22px; }
  .actions { display: flex; gap: 6px; }
  .btn {
    font-size: 11px;
    padding: 6px 12px;
    border: 1.5px solid var(--ink);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--ink);
    cursor: pointer;
    font-family: var(--font-ui);
    font-weight: 600;
    transition: transform 120ms ease, box-shadow 120ms ease, background-color 120ms ease;
  }
  .btn:hover {
    background: var(--surface-alt);
    transform: translate(-1px, -1px);
    box-shadow: 2px 2px 0 var(--ink);
  }
  .btn.primary {
    background: var(--neon);
    color: var(--ink);
    border-color: var(--ink);
    box-shadow: 2px 2px 0 var(--ink);
  }
  .btn.primary:hover {
    background: #e6ff14;
    transform: translate(-2px, -2px);
    box-shadow: 4px 4px 0 var(--ink);
  }
  .btn.danger { color: var(--danger); border-color: var(--danger); }
  .btn.danger:hover { background: var(--danger); color: var(--bg); }

  .md-body { font-size: 14px; line-height: 1.6; }
  :global(.md-body h1) { font-size: 22px; margin-top: 24px; }
  :global(.md-body h2) { font-size: 18px; margin-top: 20px; }
  :global(.md-body h3) { font-size: 15px; margin-top: 16px; }
  :global(.md-body pre) { background: var(--surface-alt); padding: 12px; overflow-x: auto; font-family: var(--font-mono); font-size: 12px; }
  :global(.md-body code) { font-family: var(--font-mono); font-size: 12px; background: var(--surface-alt); padding: 2px 4px; }
  :global(.md-body pre code) { background: none; padding: 0; }
  :global(.md-body a.wikilink) { color: var(--ink); text-decoration: underline; text-decoration-color: var(--neon); text-decoration-thickness: 2px; text-underline-offset: 2px; }

  .edit-area {
    width: 100%;
    min-height: 600px;
    border: 1px solid var(--line-strong);
    padding: 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    resize: vertical;
  }

  .evo { margin: 0; padding-left: 16px; font-size: 11px; }
  .evo li { margin-bottom: 8px; }
  .evo-head { display: flex; gap: 8px; align-items: baseline; }
  .evo-field { font-family: var(--font-mono); font-weight: 600; }
  .evo-time { color: var(--ink-faint); font-size: 10px; }
  .evo-reason { color: var(--ink-muted); margin-top: 2px; }
  .evo details summary { cursor: pointer; color: var(--ink-faint); font-size: 10px; }
  .evo details pre { background: var(--surface-alt); padding: 6px; font-family: var(--font-mono); font-size: 10px; max-height: 100px; overflow-y: auto; }

  .rel-group { margin-bottom: 12px; }
  .rel-group h4 { font-size: 10px; text-transform: uppercase; letter-spacing: 0.05em; margin: 0 0 4px; }
  .rel-list { list-style: none; margin: 0; padding: 0; }
  .rel-list li { padding: 6px 0; border-bottom: 1px solid var(--line); }
  .rel-list li:last-child { border-bottom: none; }
  .rel-list a { display: flex; align-items: center; gap: 6px; text-decoration: none; color: var(--ink); }
  .rel-list a:hover .rel-target { text-decoration: underline; }
  .rel-name { font-family: var(--font-mono); font-size: 10px; }
  .rel-target { font-size: 12px; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rel-meta { display: flex; gap: 8px; font-family: var(--font-mono); font-size: 10px; color: var(--ink-faint); margin-top: 2px; }
  .rel-meta .via { font-style: italic; }
  .rel-reason { font-size: 11px; color: var(--ink-muted); margin-top: 2px; font-style: italic; }

  .empty { color: var(--ink-faint); font-size: 11px; font-style: italic; }
  .error { border-color: var(--danger); color: var(--danger); }
  /* badges inherit from global app.css .badge / .badge-* */
</style>
