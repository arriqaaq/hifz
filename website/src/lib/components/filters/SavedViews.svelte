<script lang="ts">
  import { onMount } from 'svelte';
  import { type Filters, presetRange } from '$lib/stores/filters';

  let {
    filters,
    onApply,
  }: {
    filters: Filters;
    onApply: (next: Filters) => void;
  } = $props();

  interface SavedView {
    name: string;
    filters: Filters;
  }

  let saved = $state<SavedView[]>([]);
  let savePromptOpen = $state(false);
  let newName = $state('');
  const STORAGE_KEY = 'hifz.savedViews';

  onMount(() => {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (raw) saved = JSON.parse(raw) as SavedView[];
    } catch {
      saved = [];
    }
  });

  function persist() {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(saved));
    } catch {
      // ignore
    }
  }

  function applyToday() {
    const r = presetRange(0);
    onApply({ ...filters, since: r.since, until: r.until });
  }

  function applyDecisions() {
    onApply({ ...filters, minImportance: 7 });
  }

  function applyClear() {
    onApply({
      query: '',
      sessionId: '',
      project: '',
      obsTypes: [],
      since: '',
      until: '',
      minImportance: 0,
    });
  }

  function saveCurrent() {
    const n = newName.trim();
    if (!n) return;
    saved = [...saved.filter((s) => s.name !== n), { name: n, filters: { ...filters } }];
    persist();
    newName = '';
    savePromptOpen = false;
  }

  function applySaved(v: SavedView) {
    onApply({ ...v.filters });
  }

  function removeSaved(name: string) {
    saved = saved.filter((s) => s.name !== name);
    persist();
  }
</script>

<div class="saved">
  <span class="saved-label">Quick views</span>
  <button type="button" class="view-btn" onclick={applyToday}>Today</button>
  <button type="button" class="view-btn" onclick={applyDecisions}>Decisions ≥ 7</button>
  <button type="button" class="view-btn" onclick={applyClear}>Clear</button>

  {#each saved as v (v.name)}
    <span class="saved-pill">
      <button type="button" class="view-btn pill-btn" onclick={() => applySaved(v)}>{v.name}</button>
      <button type="button" class="pill-x" onclick={() => removeSaved(v.name)} aria-label={`Remove ${v.name}`}>✕</button>
    </span>
  {/each}

  {#if savePromptOpen}
    <span class="save-form">
      <input
        type="text"
        bind:value={newName}
        placeholder="view name"
        class="save-input"
        onkeydown={(e) => {
          if (e.key === 'Enter') saveCurrent();
          if (e.key === 'Escape') savePromptOpen = false;
        }}
      />
      <button type="button" class="view-btn" onclick={saveCurrent}>save</button>
      <button type="button" class="view-btn ghost" onclick={() => (savePromptOpen = false)}>cancel</button>
    </span>
  {:else}
    <button
      type="button"
      class="view-btn ghost"
      onclick={() => (savePromptOpen = true)}
      title="Save current filter combination"
    >★ save current</button>
  {/if}
</div>

<style>
  .saved {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding: 4px 0 8px;
    border-bottom: 1px dashed var(--border-light);
    margin-bottom: 8px;
  }

  .saved-label {
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-faint);
    margin-right: 4px;
  }

  .view-btn {
    padding: 3px 10px;
    font-size: 10px;
    font-family: var(--font-ui);
    background: var(--bg);
    border: 1px solid var(--border-light);
    color: var(--ink-secondary);
    cursor: pointer;
  }
  .view-btn:hover {
    border-color: var(--ink-muted);
    color: var(--ink);
  }
  .view-btn.ghost {
    border-style: dashed;
    color: var(--ink-faint);
  }

  .saved-pill {
    display: inline-flex;
    align-items: center;
    border: 1px solid var(--border-light);
  }
  .pill-btn { border: none; }
  .pill-x {
    border: none;
    border-left: 1px solid var(--border-light);
    background: var(--bg);
    color: var(--ink-faint);
    cursor: pointer;
    padding: 3px 6px;
    font-size: 10px;
  }
  .pill-x:hover { color: var(--accent); }

  .save-form {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .save-input {
    padding: 3px 8px;
    font-size: 10px;
    font-family: var(--font-ui);
    border: 1px solid var(--border);
    background: var(--bg);
    width: 140px;
  }
</style>
