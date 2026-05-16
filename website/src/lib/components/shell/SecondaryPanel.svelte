<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { getHealth } from '$lib/api';
  import { Layers, Workflow, Activity, Brain, GitCommit, Coins, Search } from 'lucide-svelte';
  import { shell } from '$lib/stores/shell.svelte';

  let counts = $state({
    sessions: 0,
    runs: 0,
    observations: 0,
    memories: 0,
    commits: 0,
  });

  let q = $state(page.url.searchParams.get('q') ?? '');
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  async function refreshCounts() {
    try {
      const h = await getHealth();
      counts.sessions = h.sessions ?? 0;
      counts.runs = h.runs ?? 0;
      counts.observations = h.observations ?? 0;
      counts.memories = h.memories ?? 0;
      counts.commits = h.commits ?? 0;
    } catch {
      // silent
    }
  }

  onMount(refreshCounts);

  $effect(() => {
    void shell.refreshKey;
    refreshCounts();
  });

  function onQueryInput(e: Event) {
    q = (e.currentTarget as HTMLInputElement).value;
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      const url = new URL(page.url);
      if (q) url.searchParams.set('q', q);
      else url.searchParams.delete('q');
      void goto(`${url.pathname}?${url.searchParams.toString()}`, {
        replaceState: true,
        keepFocus: true,
        noScroll: true,
      });
    }, 200);
  }

  function fmt(n: number): string {
    return n.toLocaleString();
  }

  const links = [
    { icon: Layers, href: '/sessions', label: 'Sessions', key: 'sessions' as const, color: 'var(--c-session)' },
    { icon: Workflow, href: '/runs', label: 'Runs', key: 'runs' as const, color: 'var(--c-run)' },
    { icon: Activity, href: '/observations', label: 'Observations', key: 'observations' as const, color: 'var(--c-obs)' },
    { icon: Brain, href: '/memories', label: 'Memories', key: 'memories' as const, color: 'var(--c-memory)' },
    { icon: GitCommit, href: '/commits', label: 'Commits', key: 'commits' as const, color: 'var(--c-commit)' },
  ];
</script>

{#if shell.panelOpen}
  <aside class="panel" aria-label="Secondary navigation">
    <div class="search-box">
      <Search size={14} strokeWidth={1.6} />
      <input
        type="text"
        placeholder="Quick filter…"
        value={q}
        oninput={onQueryInput}
        aria-label="Quick filter"
      />
    </div>

    <div class="section">
      <div class="section-h">Entities</div>
      {#each links as l}
        {@const Icon = l.icon}
        {@const active = page.url.pathname.startsWith(l.href)}
        <a href={l.href} class="link" class:active>
          <span class="link-icon" style={`color: ${l.color}`}><Icon size={14} strokeWidth={1.7} /></span>
          <span class="link-label">{l.label}</span>
          <span class="link-count">{fmt(counts[l.key])}</span>
        </a>
      {/each}
    </div>

    <div class="section">
      <div class="section-h">Tools</div>
      <a href="/tokens" class="link" class:active={page.url.pathname.startsWith('/tokens')}>
        <span class="link-icon" style="color: var(--ink-muted)"><Coins size={14} strokeWidth={1.7} /></span>
        <span class="link-label">Tokens</span>
      </a>
      <a href="/graph" class="link" class:active={page.url.pathname.startsWith('/graph')}>
        <span class="link-icon" style="color: var(--ink-muted)">⬡</span>
        <span class="link-label">Knowledge graph</span>
      </a>
      <button type="button" class="link link-btn" onclick={() => shell.toggleCommand()}>
        <span class="link-icon" style="color: var(--ink-muted)">⌘</span>
        <span class="link-label">Command palette</span>
        <span class="kbd">⌘K</span>
      </button>
    </div>
  </aside>
{/if}

<style>
  .panel {
    grid-area: panel;
    width: var(--panel-w);
    height: 100vh;
    background: var(--surface);
    border-right: 1px solid var(--line);
    overflow-y: auto;
    padding: 12px 10px;
    position: sticky;
    top: 0;
  }

  .search-box {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--surface-alt);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    margin-bottom: 16px;
    color: var(--ink-faint);
  }
  .search-box input {
    flex: 1;
    border: none;
    background: transparent;
    padding: 0;
    font-size: 12px;
    outline: none;
    box-shadow: none;
  }
  .search-box input:focus { box-shadow: none; }

  .section {
    margin-bottom: 18px;
  }

  .section-h {
    font-family: var(--font-ui);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-faint);
    padding: 0 8px 6px;
  }

  .link {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: var(--radius-sm);
    color: var(--ink-secondary);
    font-size: 13px;
    font-family: var(--font-ui);
    cursor: pointer;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
  }
  .link:hover {
    background: var(--surface-alt);
    color: var(--ink);
  }
  .link.active {
    background: var(--surface-alt);
    color: var(--ink);
    font-weight: 500;
  }

  .link-icon {
    width: 16px;
    display: inline-flex;
    justify-content: center;
    flex-shrink: 0;
  }

  .link-label {
    flex: 1;
  }

  .link-count {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-size: 11px;
    color: var(--ink-faint);
  }

  .kbd {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 1px 5px;
    background: var(--bg);
  }
</style>
