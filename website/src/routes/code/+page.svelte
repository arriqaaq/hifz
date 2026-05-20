<script lang="ts">
  import { onMount } from 'svelte';
  import { codeSearch, codeIndex, type CodeSearchResult, type CodeIndexReport } from '$lib/api';
  import CodeSnippet from '$lib/components/code/CodeSnippet.svelte';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import { Copy, Check, FileCode2, ArrowRight } from 'lucide-svelte';

  const LANGUAGES = ['rust', 'python', 'javascript', 'typescript', 'tsx', 'go', 'java', 'c', 'cpp'];
  const PROJECT_KEY = 'code.project';

  let project = $state('');
  let query = $state('');
  let language = $state('');
  let path = $state('');
  let groupByFile = $state(false);
  let limit = $state(20);

  let results = $state<CodeSearchResult[]>([]);
  let count = $state(0);
  let loading = $state(false);
  let searched = $state(false);
  let error = $state('');
  let copied = $state<string | null>(null);

  // index panel
  let showIndex = $state(false);
  let root = $state('');
  let indexing = $state(false);
  let report = $state<CodeIndexReport | null>(null);
  let indexError = $state('');

  onMount(() => {
    try {
      project = localStorage.getItem(PROJECT_KEY) ?? '';
    } catch {
      /* non-browser / blocked */
    }
  });

  function persistProject() {
    try {
      if (project) localStorage.setItem(PROJECT_KEY, project);
    } catch {
      /* ignore */
    }
  }

  async function doSearch(e?: SubmitEvent) {
    e?.preventDefault();
    if (!project.trim() || !query.trim()) return;
    persistProject();
    loading = true;
    error = '';
    try {
      const res = await codeSearch(project.trim(), query.trim(), {
        language: language || undefined,
        path: path.trim() || undefined,
        group_by_file: groupByFile,
        limit,
      });
      results = res.results ?? [];
      count = res.count ?? results.length;
      searched = true;
    } catch (err) {
      error = err instanceof Error ? err.message : 'Search failed';
    } finally {
      loading = false;
    }
  }

  async function doIndex() {
    if (!project.trim() || !root.trim()) return;
    persistProject();
    indexing = true;
    indexError = '';
    report = null;
    try {
      report = await codeIndex(project.trim(), root.trim());
    } catch (err) {
      indexError = err instanceof Error ? err.message : 'Index failed';
    } finally {
      indexing = false;
    }
  }

  async function copyPath(p: string) {
    try {
      await navigator.clipboard.writeText(p);
      copied = p;
      setTimeout(() => (copied = copied === p ? null : copied), 1500);
    } catch {
      /* clipboard may be blocked */
    }
  }
</script>

<h2 class="page-h">Code search</h2>
<p class="page-sub">
  Hybrid search over indexed code — file, line range, and snippet. Always code (separate from
  documents and memories).
</p>

<div class="toolbar">
  <input
    class="project-input"
    type="text"
    placeholder="Project (e.g. hifz)"
    bind:value={project}
    onchange={persistProject}
    aria-label="Project"
  />
  <button class="btn btn--small" class:btn--accent={showIndex} onclick={() => (showIndex = !showIndex)}>
    <FileCode2 size={13} strokeWidth={1.7} /> Index a repo
  </button>
</div>

{#if showIndex}
  <div class="card index-card">
    <div class="card-title">Index a repository</div>
    <div class="index-row">
      <input
        class="root-input"
        type="text"
        placeholder="/abs/path/to/repo (server-side, daemon-readable)"
        bind:value={root}
      />
      <button class="btn btn--accent btn--small" onclick={doIndex} disabled={indexing || !project.trim() || !root.trim()}>
        {indexing ? 'Indexing…' : 'Index'}
      </button>
    </div>
    {#if indexing}
      <p class="hint">Walking the repo, chunking, and embedding — large repos can take a while.</p>
    {/if}
    {#if indexError}
      <p class="err">{indexError}</p>
    {/if}
    {#if report}
      <div class="report mono">
        <span><b>{report.indexed}</b> indexed</span>
        <span><b>{report.skipped_unchanged}</b> unchanged</span>
        <span><b>{report.chunks}</b> chunks</span>
        <span><b>{report.symbols}</b> symbols</span>
        <span class:err-inline={report.errors > 0}><b>{report.errors}</b> errors</span>
      </div>
    {/if}
  </div>
{/if}

<form class="search-row" onsubmit={doSearch}>
  <input
    type="text"
    class="search-input"
    placeholder="Search code… (e.g. session refresh token)"
    bind:value={query}
  />
  <button type="submit" class="btn btn--accent btn--small" disabled={loading || !project.trim() || !query.trim()}>
    Search
  </button>
</form>

<div class="filters">
  <select bind:value={language} onchange={() => searched && doSearch()} title="Filter by language">
    <option value="">All languages</option>
    {#each LANGUAGES as l}
      <option value={l}>{l}</option>
    {/each}
  </select>
  <input class="filter-input" type="text" placeholder="Path contains…" bind:value={path} />
  <label class="check-label">
    <input type="checkbox" bind:checked={groupByFile} onchange={() => searched && doSearch()} />
    Group by file
  </label>
  <select bind:value={limit} onchange={() => searched && doSearch()} title="Result limit">
    <option value={10}>10 results</option>
    <option value={20}>20 results</option>
    <option value={50}>50 results</option>
  </select>
</div>

{#if error}
  <div class="card" style="border-color: var(--danger);">
    <p style="color: var(--danger); margin: 0;">{error}</p>
  </div>
{:else if loading}
  <LoadingSpinner />
{:else if searched && results.length === 0}
  <p class="empty">
    No matches{project ? ` in “${project}”` : ''}. If this project isn't indexed yet, use the
    <b>Index a repo</b> panel above or run <code>hifz_code_index</code>.
  </p>
{:else if results.length > 0}
  <p class="result-meta mono">{count} {count === 1 ? 'hit' : 'hits'}</p>
  <div class="hits">
    {#each results as h (h.id)}
      <div class="card hit">
        <div class="hit-head">
          <button class="path" onclick={() => copyPath(h.path)} title="Copy path">
            {h.path}
            {#if copied === h.path}
              <Check size={12} strokeWidth={2} />
            {:else}
              <Copy size={12} strokeWidth={1.7} />
            {/if}
          </button>
          {#if h.language}<span class="badge badge-yellow">{h.language}</span>{/if}
          <span class="lines mono">L{h.start_line}–L{h.end_line}</span>
          <span class="score mono">{h.score.toFixed(3)}</span>
        </div>
        <CodeSnippet code={h.snippet} language={h.language} />
      </div>
    {/each}
  </div>
{:else}
  <p class="empty">
    Enter a project and a query to search its indexed code. Memories link to these chunks via
    <code>hifz_link_code</code>.
  </p>
{/if}

<!-- Linking legend — mirrors docs/img/ui-code-search.svg. Static explainer; the
     memory→code edges are created agent-side via hifz_link_code. -->
<div class="legend">
  <span class="mem-chip">memory</span>
  <span class="legend-label">JWT refresh in Redis</span>
  <ArrowRight size={13} strokeWidth={1.8} class="legend-arrow" />
  <span class="legend-rel mono">references</span>
  <span class="file-chip mono">src/auth/session.rs:42-71</span>
  <span class="legend-note"><code>hifz_link_code</code> · links re-anchor on re-index</span>
</div>

<style>
  .page-h {
    margin: 0 0 4px;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  .page-sub {
    margin: 0 0 18px;
    color: var(--ink-muted);
    font-size: 13px;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
  }
  .project-input {
    width: 220px;
  }
  .toolbar .btn {
    margin-left: auto;
  }

  .index-card {
    margin-bottom: 14px;
  }
  .index-row {
    display: flex;
    gap: 8px;
  }
  .root-input {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .report {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    margin-top: 10px;
    font-size: 12px;
    color: var(--ink-muted);
  }
  .report b {
    color: var(--ink);
  }
  .err,
  .err-inline {
    color: var(--danger);
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--ink-faint);
  }

  .search-row {
    display: flex;
    gap: 0;
    border: 1px solid var(--line-strong);
    margin-bottom: 10px;
    max-width: 900px;
  }
  .search-input {
    flex: 1;
    border: none;
    padding: 12px 16px;
    font-size: 14px;
    font-family: var(--font-body);
  }
  .search-input:focus {
    border: none;
    box-shadow: none;
  }
  .search-row .btn {
    border: none;
    border-left: 1px solid var(--line-strong);
    border-radius: 0;
  }

  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-bottom: 16px;
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

  .result-meta {
    font-size: 11px;
    color: var(--ink-faint);
    margin-bottom: 12px;
  }

  .hits {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 900px;
  }
  .hit {
    padding: 12px 14px;
  }
  .hit-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }
  .path {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: var(--font-mono);
    font-size: 13px;
    font-weight: 700;
    color: var(--ink);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
  }
  .path:hover {
    text-decoration: underline;
    text-decoration-color: var(--neon);
    text-decoration-thickness: 2px;
    text-underline-offset: 2px;
  }
  .path :global(svg) {
    color: var(--ink-faint);
  }
  .lines {
    font-size: 11px;
    color: var(--ink-muted);
  }
  .score {
    margin-left: auto;
    font-size: 11px;
    color: var(--ink-faint);
  }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
    max-width: 640px;
    margin: 0 auto;
  }
  code {
    font-family: var(--font-mono);
    font-size: 0.92em;
    background: var(--surface-alt);
    padding: 1px 4px;
    border-radius: 4px;
  }

  /* linking legend */
  .legend {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 24px;
    padding-top: 14px;
    border-top: 1px solid var(--line);
    font-size: 12px;
    color: var(--ink-muted);
    max-width: 900px;
  }
  .mem-chip {
    font-family: var(--font-mono);
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.04em;
    color: var(--fg-on-dark);
    background: var(--c-memory);
    border: 1px solid var(--ink);
    padding: 2px 8px;
    clip-path: polygon(8% 0, 92% 0, 100% 50%, 92% 100%, 8% 100%, 0 50%);
  }
  .legend-rel {
    font-size: 9px;
    color: var(--blue);
  }
  .file-chip {
    font-size: 11px;
    color: var(--cyan);
    background: color-mix(in srgb, var(--cyan) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--cyan) 28%, transparent);
    border-radius: var(--radius-sm);
    padding: 2px 8px;
  }
  .legend-note {
    color: var(--ink-faint);
    margin-left: 4px;
  }
  .legend :global(.legend-arrow) {
    color: var(--blue);
  }
</style>
