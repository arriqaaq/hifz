<script lang="ts">
  import {
    type Filters,
    OBS_TYPES,
    DATE_PRESETS,
    presetRange,
    isEmpty,
  } from '$lib/stores/filters';
  import SessionPicker from './SessionPicker.svelte';
  import SavedViews from './SavedViews.svelte';

  let {
    filters,
    onChange,
    projects = [],
  }: {
    filters: Filters;
    onChange: (next: Filters) => void;
    projects?: string[];
  } = $props();

  let typesOpen = $state(false);

  function update<K extends keyof Filters>(key: K, value: Filters[K]) {
    onChange({ ...filters, [key]: value });
  }

  function toggleType(t: string) {
    const cur = filters.obsTypes;
    const next = cur.includes(t) ? cur.filter((x) => x !== t) : [...cur, t];
    update('obsTypes', next);
  }

  function applyPreset(days: number) {
    const r = presetRange(days);
    onChange({ ...filters, since: r.since, until: r.until });
  }

  function clearAll() {
    onChange({
      query: '',
      sessionId: '',
      project: '',
      obsTypes: [],
      since: '',
      until: '',
      minImportance: 0,
    });
  }
</script>

<div class="filter-bar">
  <SavedViews {filters} onApply={onChange} />

  <div class="row row-1">
    <span class="row-label">Filters</span>
    <SessionPicker
      value={filters.sessionId}
      onChange={(id) => update('sessionId', id)}
    />

    {#if projects.length > 0}
      <select
        class="select"
        value={filters.project}
        onchange={(e) => update('project', (e.currentTarget as HTMLSelectElement).value)}
      >
        <option value="">All projects</option>
        {#each projects as p}
          <option value={p}>{p}</option>
        {/each}
      </select>
    {/if}

    <div class="types">
      {#each filters.obsTypes as t (t)}
        <button type="button" class="chip chip-active" onclick={() => toggleType(t)}>
          {t} <span class="x">✕</span>
        </button>
      {/each}
      <button type="button" class="chip chip-add" onclick={() => (typesOpen = !typesOpen)}>
        + type
      </button>
      {#if typesOpen}
        <div class="types-menu">
          {#each OBS_TYPES as t}
            {@const active = filters.obsTypes.includes(t)}
            <button
              type="button"
              class="types-menu-item"
              class:active
              onclick={() => toggleType(t)}
            >
              {active ? '✓' : ' '} {t}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    {#if !isEmpty(filters)}
      <button type="button" class="clear" onclick={clearAll}>Clear all</button>
    {/if}
  </div>

  <div class="row row-2">
    <span class="row-label">Date</span>
    <input
      type="date"
      class="date-input"
      value={filters.since}
      onchange={(e) => update('since', (e.currentTarget as HTMLInputElement).value)}
    />
    <span class="arrow">→</span>
    <input
      type="date"
      class="date-input"
      value={filters.until}
      onchange={(e) => update('until', (e.currentTarget as HTMLInputElement).value)}
    />
    {#each DATE_PRESETS as p}
      <button type="button" class="preset" onclick={() => applyPreset(p.days)}>{p.label}</button>
    {/each}

    <span class="row-label" style="margin-left: 16px">Importance ≥</span>
    <input
      type="range"
      min="0"
      max="10"
      class="slider"
      value={filters.minImportance}
      oninput={(e) => update('minImportance', Number((e.currentTarget as HTMLInputElement).value))}
    />
    <span class="imp-value">{filters.minImportance}</span>
  </div>
</div>

<style>
  .filter-bar {
    border: 1px solid var(--line-strong);
    background: var(--bg);
    margin-bottom: 16px;
    padding: 10px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .row-label {
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-faint);
    min-width: 60px;
  }

  .select {
    padding: 4px 8px;
    font-size: 11px;
    font-family: var(--font-ui);
    background: var(--bg);
    border: 1px solid var(--line-strong);
    color: var(--ink);
    cursor: pointer;
  }

  .types {
    position: relative;
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
  }

  .chip {
    padding: 3px 10px;
    font-size: 11px;
    font-family: var(--font-ui);
    background: transparent;
    border: 1px solid var(--line-strong);
    color: var(--ink);
    cursor: pointer;
  }
  .chip-active {
    background: var(--surface-alt);
    border-color: var(--ink-muted);
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .chip-active .x {
    color: var(--ink-faint);
    font-size: 10px;
  }
  .chip-add {
    border-style: dashed;
    color: var(--ink-muted);
  }
  .chip-add:hover {
    border-color: var(--ink-muted);
    color: var(--ink);
  }

  .types-menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 10;
    background: var(--bg);
    border: 1px solid var(--line-strong);
    box-shadow: 4px 4px 0 0 var(--line);
    min-width: 180px;
    display: flex;
    flex-direction: column;
    padding: 4px;
  }
  .types-menu-item {
    background: none;
    border: none;
    padding: 4px 10px;
    text-align: left;
    font-size: 11px;
    font-family: var(--font-ui);
    color: var(--ink);
    cursor: pointer;
  }
  .types-menu-item:hover { background: var(--surface-alt); }
  .types-menu-item.active { color: var(--accent); font-weight: 600; }

  .clear {
    margin-left: auto;
    background: none;
    border: none;
    cursor: pointer;
    font-family: var(--font-ui);
    font-size: 10px;
    color: var(--accent);
    text-decoration: underline;
  }

  .date-input {
    padding: 3px 8px;
    font-size: 11px;
    font-family: var(--font-mono);
    background: var(--bg);
    border: 1px solid var(--line-strong);
    color: var(--ink);
  }
  .arrow { color: var(--ink-faint); font-size: 11px; }

  .preset {
    padding: 3px 10px;
    font-size: 10px;
    font-family: var(--font-ui);
    background: transparent;
    border: 1px solid var(--line);
    color: var(--ink-muted);
    cursor: pointer;
  }
  .preset:hover { color: var(--ink); border-color: var(--line-strong); }

  .slider {
    width: 120px;
    cursor: pointer;
  }
  .imp-value {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink);
    min-width: 14px;
    text-align: center;
  }
</style>
