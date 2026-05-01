<script lang="ts">
  import { onMount } from 'svelte';
  import { getSessions } from '$lib/api';
  import type { Session } from '$lib/types';

  let { value, onChange }: { value: string; onChange: (id: string) => void } = $props();

  let sessions = $state<Session[]>([]);
  let open = $state(false);
  let filter = $state('');
  let inputEl = $state<HTMLInputElement | null>(null);
  const listId = `session-picker-list-${Math.random().toString(36).slice(2, 8)}`;

  onMount(async () => {
    try {
      const r = await getSessions(200);
      sessions = r.sessions;
    } catch {
      sessions = [];
    }
  });

  function extractId(id: unknown): string {
    if (typeof id === 'string') return id;
    if (id && typeof id === 'object') {
      const o = id as Record<string, unknown>;
      if (typeof o.key === 'string') return o.key;
      if (o.key && typeof o.key === 'object' && 'String' in (o.key as Record<string, unknown>)) {
        return (o.key as { String: string }).String;
      }
    }
    return String(id);
  }

  let selectedLabel = $derived.by(() => {
    if (!value) return '';
    const s = sessions.find((s) => extractId(s.id) === value);
    if (!s) return value.slice(0, 20);
    return `${s.project ?? '?'} · ${s.name ?? extractId(s.id).slice(0, 8)}`;
  });

  let filtered = $derived.by(() => {
    const q = filter.toLowerCase();
    if (!q) return sessions.slice(0, 50);
    return sessions
      .filter((s) => {
        const id = extractId(s.id);
        return (
          id.toLowerCase().includes(q) ||
          (s.name ?? '').toLowerCase().includes(q) ||
          (s.project ?? '').toLowerCase().includes(q)
        );
      })
      .slice(0, 50);
  });

  function pick(id: string) {
    onChange(id);
    open = false;
    filter = '';
  }

  function clear() {
    onChange('');
    filter = '';
  }

  function toggleOpen() {
    open = !open;
    if (open) setTimeout(() => inputEl?.focus(), 0);
  }

  function handleKey(e: KeyboardEvent) {
    if (e.key === 'Escape') open = false;
  }

  function fmtDate(ts?: string): string {
    if (!ts) return '';
    try {
      return new Date(ts).toLocaleDateString([], { month: 'short', day: 'numeric' });
    } catch {
      return '';
    }
  }
</script>

<div
  class="picker"
  onkeydown={handleKey}
  role="combobox"
  aria-expanded={open}
  aria-controls={listId}
  aria-haspopup="listbox"
  tabindex="-1"
>
  {#if value}
    <span class="chip">
      <span class="chip-label">{selectedLabel}</span>
      <button type="button" class="chip-x" onclick={clear} aria-label="Clear session filter">✕</button>
    </span>
  {:else}
    <button type="button" class="trigger" onclick={toggleOpen}>
      + Session
    </button>
  {/if}

  {#if open && !value}
    <div class="dropdown">
      <input
        bind:this={inputEl}
        type="text"
        placeholder="Search sessions…"
        bind:value={filter}
        class="dropdown-search"
      />
      <div id={listId} class="dropdown-list" role="listbox">
        {#if filtered.length === 0}
          <p class="dropdown-empty">No sessions</p>
        {:else}
          {#each filtered as s (extractId(s.id))}
            {@const id = extractId(s.id)}
            <button type="button" class="dropdown-item" onclick={() => pick(id)}>
              <span class="item-project">{s.project ?? '—'}</span>
              <span class="item-name">{s.name ?? id.slice(0, 12)}</span>
              <span class="item-date">{fmtDate(s.started_at)}</span>
            </button>
          {/each}
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    display: inline-block;
  }

  .trigger {
    padding: 4px 10px;
    font-size: 11px;
    font-family: var(--font-ui);
    background: transparent;
    border: 1px dashed var(--line-strong);
    color: var(--ink-muted);
    cursor: pointer;
  }
  .trigger:hover {
    border-color: var(--ink-muted);
    color: var(--ink);
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 4px 3px 10px;
    background: var(--surface-alt);
    border: 1px solid var(--line-strong);
    font-size: 11px;
    font-family: var(--font-ui);
  }
  .chip-label {
    font-weight: 500;
  }
  .chip-x {
    background: none;
    border: none;
    cursor: pointer;
    padding: 0 4px;
    color: var(--ink-faint);
    font-size: 11px;
  }
  .chip-x:hover { color: var(--accent); }

  .dropdown {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 10;
    width: 320px;
    max-height: 360px;
    background: var(--bg);
    border: 1px solid var(--line-strong);
    box-shadow: 4px 4px 0 0 var(--line);
    display: flex;
    flex-direction: column;
  }

  .dropdown-search {
    padding: 8px 12px;
    border: none;
    border-bottom: 1px solid var(--line);
    font-size: 12px;
    font-family: var(--font-body);
    outline: none;
  }

  .dropdown-list {
    overflow-y: auto;
    flex: 1;
    max-height: 300px;
  }

  .dropdown-item {
    width: 100%;
    display: grid;
    grid-template-columns: 80px 1fr 60px;
    gap: 8px;
    align-items: center;
    padding: 6px 12px;
    background: none;
    border: none;
    border-bottom: 1px solid var(--line);
    cursor: pointer;
    text-align: left;
    font-family: var(--font-ui);
    font-size: 11px;
  }
  .dropdown-item:hover {
    background: var(--surface-alt);
  }

  .item-project {
    color: var(--ink-muted);
    font-family: var(--font-mono);
    font-size: 10px;
  }
  .item-name {
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item-date {
    color: var(--ink-faint);
    font-family: var(--font-mono);
    font-size: 10px;
    text-align: right;
  }

  .dropdown-empty {
    padding: 16px;
    margin: 0;
    text-align: center;
    color: var(--ink-faint);
    font-size: 11px;
  }
</style>
