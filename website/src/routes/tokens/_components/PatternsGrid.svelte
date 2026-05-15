<script lang="ts">
  import type { UsagePattern } from '$lib/api';
  import { AlertTriangle, Info, Circle } from 'lucide-svelte';

  type Props = { patterns: UsagePattern[] };
  let { patterns }: Props = $props();

  function iconFor(kind: string) {
    if (kind === 'warning') return AlertTriangle;
    if (kind === 'info') return Info;
    return Circle;
  }
</script>

{#if patterns.length === 0}
  <div class="empty">No patterns detected. Either there isn't enough data yet, or your usage looks balanced.</div>
{:else}
  <div class="grid">
    {#each patterns as p}
      {@const Icon = iconFor(p.kind)}
      <div class={`card kind-${p.kind}`}>
        <div class="head">
          <Icon size={14} strokeWidth={1.8} />
          <h4>{p.title}</h4>
        </div>
        <p class="body">{p.body}</p>
        {#if p.action}
          <p class="action">{p.action}</p>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 12px;
    margin-bottom: 14px;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    padding: 12px;
  }
  .card.kind-warning {
    border-left: 3px solid var(--c-bug, #c4453d);
  }
  .card.kind-info {
    border-left: 3px solid var(--c-obs, #7b9bd4);
  }
  .card.kind-neutral {
    border-left: 3px solid var(--line-strong, #888);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 6px;
    color: var(--ink-muted);
  }
  .head h4 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    color: var(--ink);
  }
  .body {
    margin: 0;
    font-size: 12px;
    color: var(--ink-muted);
    line-height: 1.4;
  }
  .action {
    margin: 6px 0 0;
    font-size: 11px;
    color: var(--ink-faint);
    font-style: italic;
    line-height: 1.4;
  }
  .empty {
    padding: 18px;
    text-align: center;
    color: var(--ink-faint);
    font-size: 12px;
    border: 1px dashed var(--line);
    border-radius: var(--radius-sm);
    margin-bottom: 14px;
  }
</style>
