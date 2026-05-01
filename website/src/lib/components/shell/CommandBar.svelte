<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { getSessions, searchMemories, searchObservations } from '$lib/api';
  import type { Session, Memory, Observation } from '$lib/types';
  import { shell } from '$lib/stores/shell.svelte';
  import { extractId, entityHref } from '$lib/components/entity/entityHelpers';
  import EntityChip from '$lib/components/entity/EntityChip.svelte';
  import { Search, X } from 'lucide-svelte';

  let q = $state('');
  let inputEl = $state<HTMLInputElement | null>(null);
  let activeIdx = $state(0);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  let allSessions = $state<Session[]>([]);
  let memoryHits = $state<Memory[]>([]);
  let obsHits = $state<Observation[]>([]);

  const ACTIONS: { id: string; label: string; hint?: string; run: () => void }[] = [
    {
      id: 'panel',
      label: 'Toggle sidebar',
      hint: '[',
      run: () => {
        shell.togglePanel();
        close();
      },
    },
    {
      id: 'graph',
      label: 'Open knowledge graph',
      run: () => {
        void goto('/graph');
        close();
      },
    },
    {
      id: 'sessions',
      label: 'Browse sessions',
      run: () => {
        void goto('/sessions');
        close();
      },
    },
  ];

  onMount(async () => {
    try {
      const r = await getSessions(50);
      allSessions = r.sessions;
    } catch {
      allSessions = [];
    }
  });

  $effect(() => {
    if (shell.commandOpen) {
      setTimeout(() => inputEl?.focus(), 0);
      activeIdx = 0;
    } else {
      q = '';
      memoryHits = [];
      obsHits = [];
    }
  });

  $effect(() => {
    const query = q.trim();
    if (debounceTimer) clearTimeout(debounceTimer);
    if (!query) {
      memoryHits = [];
      obsHits = [];
      return;
    }
    debounceTimer = setTimeout(async () => {
      try {
        const [m, o] = await Promise.all([
          searchMemories(query, 6).catch(() => ({ memories: [] as Memory[], count: 0 })),
          searchObservations({ query, limit: 6 }).catch(() => ({ observations: [] as Observation[], count: 0 })),
        ]);
        memoryHits = m.memories;
        obsHits = o.observations;
      } catch {
        memoryHits = [];
        obsHits = [];
      }
    }, 150);
  });

  let filteredSessions = $derived.by(() => {
    const query = q.trim().toLowerCase();
    if (!query) return allSessions.slice(0, 6);
    return allSessions
      .filter((s) => {
        const id = extractId(s.id).toLowerCase();
        return (
          id.includes(query) ||
          (s.name ?? '').toLowerCase().includes(query) ||
          (s.project ?? '').toLowerCase().includes(query)
        );
      })
      .slice(0, 6);
  });

  let items = $derived.by(() => {
    type Item =
      | { kind: 'session'; data: Session }
      | { kind: 'memory'; data: Memory }
      | { kind: 'observation'; data: Observation }
      | { kind: 'action'; data: (typeof ACTIONS)[number] };
    const out: Item[] = [];
    for (const s of filteredSessions) out.push({ kind: 'session', data: s });
    for (const m of memoryHits) out.push({ kind: 'memory', data: m });
    for (const o of obsHits) out.push({ kind: 'observation', data: o });
    for (const a of ACTIONS) out.push({ kind: 'action', data: a });
    return out;
  });

  function close() {
    shell.commandOpen = false;
  }

  function activate(item: (typeof items)[number]) {
    if (item.kind === 'session') {
      const id = extractId(item.data.id).replace(/^session:/, '');
      const href = entityHref('session', id);
      if (href) void goto(href);
      close();
      return;
    }
    if (item.kind === 'observation') {
      shell.openDrawer({
        kind: 'observation',
        id: extractId(item.data.id),
        data: item.data,
      });
      close();
      return;
    }
    if (item.kind === 'memory') {
      shell.openDrawer({
        kind: 'memory',
        id: extractId(item.data.id),
        data: item.data,
      });
      close();
      return;
    }
    if (item.kind === 'action') {
      item.data.run();
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      activeIdx = Math.min(items.length - 1, activeIdx + 1);
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      activeIdx = Math.max(0, activeIdx - 1);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      const it = items[activeIdx];
      if (it) activate(it);
    }
  }

  let sectionStart = $derived.by(() => {
    let i = 0;
    const sessionsStart = i;
    i += filteredSessions.length;
    const memoriesStart = i;
    i += memoryHits.length;
    const obsStart = i;
    i += obsHits.length;
    const actionsStart = i;
    return { sessionsStart, memoriesStart, obsStart, actionsStart };
  });
</script>

{#if shell.commandOpen}
  <div
    class="overlay"
    role="dialog"
    aria-modal="true"
    aria-label="Command palette"
    onclick={close}
    onkeydown={onKey}
    tabindex="-1"
  >
    <div class="palette" role="presentation" onclick={(e) => e.stopPropagation()}>
      <div class="search-row">
        <Search size={14} strokeWidth={1.6} />
        <input
          bind:this={inputEl}
          type="text"
          bind:value={q}
          placeholder="Search sessions, observations, memories…"
          aria-label="Search"
        />
        <kbd>esc</kbd>
        <button type="button" class="close-btn" onclick={close} aria-label="Close">
          <X size={14} strokeWidth={1.6} />
        </button>
      </div>

      <div class="list" role="listbox">
        {#if items.length === 0}
          <p class="empty">Type to search.</p>
        {:else}
          {#if filteredSessions.length > 0}
            <div class="section-h">Sessions</div>
            {#each filteredSessions as s, i}
              {@const idx = sectionStart.sessionsStart + i}
              {@const sid = extractId(s.id)}
              <button
                type="button"
                class="item"
                class:active={activeIdx === idx}
                onmouseenter={() => (activeIdx = idx)}
                onclick={() => activate({ kind: 'session', data: s })}
              >
                <EntityChip kind="session" id={sid} size="sm" href={null} />
                <span class="item-name">{s.name ?? s.project ?? sid}</span>
                <span class="item-meta">{s.project ?? ''}</span>
              </button>
            {/each}
          {/if}

          {#if memoryHits.length > 0}
            <div class="section-h">Memories</div>
            {#each memoryHits as m, i}
              {@const idx = sectionStart.memoriesStart + i}
              <button
                type="button"
                class="item"
                class:active={activeIdx === idx}
                onmouseenter={() => (activeIdx = idx)}
                onclick={() => activate({ kind: 'memory', data: m })}
              >
                <EntityChip kind="memory" id={extractId(m.id)} size="sm" href={null} />
                <span class="item-name">{m.title}</span>
                <span class="item-meta">{m.category}</span>
              </button>
            {/each}
          {/if}

          {#if obsHits.length > 0}
            <div class="section-h">Observations</div>
            {#each obsHits as o, i}
              {@const idx = sectionStart.obsStart + i}
              <button
                type="button"
                class="item"
                class:active={activeIdx === idx}
                onmouseenter={() => (activeIdx = idx)}
                onclick={() => activate({ kind: 'observation', data: o })}
              >
                <EntityChip kind="observation" id={extractId(o.id)} size="sm" href={null} />
                <span class="item-name">{o.title}</span>
                <span class="item-meta">{o.obs_type}</span>
              </button>
            {/each}
          {/if}

          <div class="section-h">Actions</div>
          {#each ACTIONS as a, i}
            {@const idx = sectionStart.actionsStart + i}
            <button
              type="button"
              class="item"
              class:active={activeIdx === idx}
              onmouseenter={() => (activeIdx = idx)}
              onclick={() => activate({ kind: 'action', data: a })}
            >
              <span class="item-icon">⌘</span>
              <span class="item-name">{a.label}</span>
              {#if a.hint}<span class="item-meta kbd">{a.hint}</span>{/if}
            </button>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(17, 17, 17, 0.45);
    z-index: 100;
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
  }

  .palette {
    width: min(620px, 90vw);
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .search-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
    color: var(--ink-muted);
  }
  .search-row input {
    flex: 1;
    border: none;
    background: transparent;
    padding: 0;
    font-size: 14px;
    color: var(--ink);
    outline: none;
    box-shadow: none;
  }
  .search-row input:focus { box-shadow: none; }

  kbd {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 1px 5px;
    background: var(--bg);
  }

  .close-btn {
    width: 24px;
    height: 24px;
    border-radius: 4px;
    background: transparent;
    border: none;
    cursor: pointer;
    color: var(--ink-faint);
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .close-btn:hover { background: var(--surface-alt); color: var(--ink); }

  .list {
    max-height: 60vh;
    overflow-y: auto;
    padding: 6px 0;
  }

  .section-h {
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-faint);
    padding: 8px 12px 4px;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    width: 100%;
    background: transparent;
    border: none;
    text-align: left;
    cursor: pointer;
    color: var(--ink-secondary);
    font-family: var(--font-ui);
    font-size: 13px;
  }
  .item.active {
    background: var(--surface-alt);
    color: var(--ink);
  }

  .item-icon {
    font-family: var(--font-mono);
    color: var(--ink-faint);
    width: 16px;
    text-align: center;
  }

  .item-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .item-meta {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
  }

  .empty {
    padding: 20px;
    text-align: center;
    color: var(--ink-faint);
    font-family: var(--font-ui);
    font-size: 12px;
    margin: 0;
  }
</style>
