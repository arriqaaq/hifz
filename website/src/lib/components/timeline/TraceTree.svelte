<script lang="ts">
  import type { Session, Run, Observation } from '$lib/types';
  import EntityChip from '$lib/components/entity/EntityChip.svelte';
  import { extractId, kindForObsType } from '$lib/components/entity/entityHelpers';
  import { ChevronDown, ChevronRight } from 'lucide-svelte';

  type Props = {
    session: Session | null;
    runs: Run[];
    observations: Observation[];
    selectedId?: string;
    onSelect?: (kind: 'session' | 'run' | 'observation', id: string, data: unknown) => void;
  };

  let { session, runs, observations, selectedId, onSelect }: Props = $props();

  let collapsed = $state<Set<string>>(new Set());

  function toggle(id: string) {
    if (collapsed.has(id)) collapsed.delete(id);
    else collapsed.add(id);
    collapsed = new Set(collapsed);
  }

  function obsIdsForRun(run: Run): string[] {
    return (run.observation_ids ?? []).map((x) => extractId(x));
  }

  let groups = $derived.by(() => {
    const inRun = new Set<string>();
    const groupedRuns: Array<{ run: Run; runId: string; obs: Observation[] }> = [];
    for (const run of runs) {
      const ids = obsIdsForRun(run);
      const set = new Set(ids);
      const matched = observations.filter((o) => set.has(extractId(o.id)));
      matched.forEach((o) => inRun.add(extractId(o.id)));
      groupedRuns.push({ run, runId: extractId(run.id), obs: matched });
    }
    const orphans = observations.filter((o) => !inRun.has(extractId(o.id)));
    return { runs: groupedRuns, orphans };
  });

  function fmtTime(ts: string): string {
    return new Date(ts).toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
  }

  function selectSession() {
    if (session) onSelect?.('session', extractId(session.id), session);
  }
  function selectRun(run: Run) {
    onSelect?.('run', extractId(run.id), run);
  }
  function selectObs(obs: Observation) {
    onSelect?.('observation', extractId(obs.id), obs);
  }
</script>

<div class="trace-tree" role="tree">
  {#if session}
    {@const sid = extractId(session.id)}
    <button
      type="button"
      class="row row-session"
      class:selected={selectedId === sid}
      onclick={selectSession}
    >
      <span class="caret"></span>
      <EntityChip kind="session" id={sid} size="sm" href={null} />
      <span class="row-label">{session.name ?? session.project ?? '—'}</span>
    </button>
  {/if}

  {#each groups.runs as { run, runId, obs }}
    {@const isCollapsed = collapsed.has(runId)}
    <div class="row row-run" class:selected={selectedId === runId}>
      <button
        type="button"
        class="caret-btn"
        onclick={() => toggle(runId)}
        aria-label={isCollapsed ? 'Expand' : 'Collapse'}
      >
        {#if isCollapsed}
          <ChevronRight size={12} strokeWidth={1.6} />
        {:else}
          <ChevronDown size={12} strokeWidth={1.6} />
        {/if}
      </button>
      <button type="button" class="row-body" onclick={() => selectRun(run)}>
        <EntityChip kind="run" id={runId} size="sm" href={null} />
        <span class="row-label">{run.prompt}</span>
        <span class="row-meta">{obs.length}</span>
      </button>
    </div>

    {#if !isCollapsed}
      {#each obs as o}
        {@const oid = extractId(o.id)}
        <button
          type="button"
          class="row row-obs"
          class:selected={selectedId === oid}
          class:muted={o.title === 'unknown call' || o.title === 'User submitted a prompt.'}
          onclick={() => selectObs(o)}
        >
          <span class="caret"></span>
          <EntityChip
            kind={kindForObsType(o.obs_type)}
            id={oid}
            label={o.obs_type}
            size="sm"
            href={null}
          />
          <span class="row-time">{fmtTime(o.timestamp)}</span>
          <span class="row-label">{o.title}</span>
          {#if o.importance >= 7}
            <span class="row-imp">★</span>
          {/if}
        </button>
      {/each}
    {/if}
  {/each}

  {#if groups.orphans.length > 0}
    <div class="orphans-h">Other observations</div>
    {#each groups.orphans as o}
      {@const oid = extractId(o.id)}
      <button
        type="button"
        class="row row-obs"
        class:selected={selectedId === oid}
        class:muted={o.title === 'unknown call' || o.title === 'User submitted a prompt.'}
        onclick={() => selectObs(o)}
      >
        <span class="caret"></span>
        <EntityChip
          kind={kindForObsType(o.obs_type)}
          id={oid}
          label={o.obs_type}
          size="sm"
          href={null}
        />
        <span class="row-time">{fmtTime(o.timestamp)}</span>
        <span class="row-label">{o.title}</span>
      </button>
    {/each}
  {/if}
</div>

<style>
  .trace-tree {
    display: flex;
    flex-direction: column;
    gap: 1px;
    font-family: var(--font-ui);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    background: transparent;
    border: none;
    border-radius: var(--radius-sm);
    text-align: left;
    cursor: pointer;
    color: var(--ink-secondary);
    font-size: 12px;
    width: 100%;
  }
  .row:hover {
    background: var(--surface-alt);
  }
  .row.selected {
    background: color-mix(in srgb, var(--neon) 22%, transparent);
    color: var(--ink);
  }
  .row.muted {
    opacity: 0.5;
  }

  .row-session {
    font-weight: 500;
    color: var(--ink);
  }

  .row-run {
    padding-left: 8px;
    cursor: default;
  }
  .row-run.selected {
    background: color-mix(in srgb, var(--neon) 22%, transparent);
  }

  .row-body {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    background: transparent;
    border: none;
    padding: 0;
    cursor: pointer;
    color: inherit;
    font-size: inherit;
    font-family: inherit;
    text-align: left;
    overflow: hidden;
  }
  .row-body:hover { color: var(--ink); }

  .row-obs {
    padding-left: 28px;
    font-size: 11.5px;
  }

  .caret,
  .caret-btn {
    width: 14px;
    height: 14px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--ink-faint);
    border: none;
    background: transparent;
    cursor: pointer;
    padding: 0;
  }
  .caret-btn:hover { color: var(--ink); }

  .row-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-time {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    font-size: 10.5px;
    color: var(--ink-faint);
    flex-shrink: 0;
  }

  .row-meta {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--ink-faint);
  }

  .row-imp {
    color: var(--c-obs);
    font-size: 11px;
    flex-shrink: 0;
  }

  .orphans-h {
    margin-top: 10px;
    padding: 4px 8px;
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-faint);
  }
</style>
