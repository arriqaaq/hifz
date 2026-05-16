<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchSessionUsage, type SessionUsageView } from '$lib/api';
  import { AlertTriangle, Info, Circle, ChevronDown, ChevronRight } from 'lucide-svelte';

  type Props = { sessionId: string };
  let { sessionId }: Props = $props();

  let view = $state<SessionUsageView | null>(null);
  let loading = $state<boolean>(true);
  let error = $state<string>('');
  let expanded = $state<boolean>(false);

  onMount(async () => {
    try {
      view = await fetchSessionUsage(sessionId);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load usage';
    } finally {
      loading = false;
    }
  });

  function fmt(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 10_000) return `${Math.round(n / 1_000)}K`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
    return String(n);
  }

  function shortTime(ts: string): string {
    try {
      return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    } catch {
      return ts;
    }
  }

  let segments = $derived.by(() => {
    if (!view) return [] as { key: string; value: number; color: string }[];
    const t = view.totals;
    const out: { key: string; value: number; color: string }[] = [];
    out.push({ key: 'input', value: t.input, color: 'var(--c-mem, #4a8c5b)' });
    const palette = ['var(--c-run, #d4a155)', 'var(--c-obs, #7b9bd4)', '#888'];
    Object.keys(t.breakdown ?? {})
      .sort()
      .forEach((k, i) => {
        out.push({ key: k, value: t.breakdown[k] ?? 0, color: palette[i % palette.length] });
      });
    out.push({ key: 'output', value: t.output, color: 'var(--c-fix, #b478b8)' });
    return out;
  });

  let totalForBar = $derived(
    segments.reduce((s, x) => s + x.value, 0) || 1,
  );

  function iconFor(kind: string) {
    if (kind === 'warning') return AlertTriangle;
    if (kind === 'info') return Info;
    return Circle;
  }
</script>

<section class="panel">
  {#if loading}
    <div class="placeholder">Loading token usage…</div>
  {:else if error}
    <div class="placeholder error">{error}</div>
  {:else if !view || view.call_count === 0}
    <div class="placeholder">
      <strong>No recorded token data for this session.</strong>
      <div class="hint">
        This session predates the token tracker or hasn't been backfilled —
        nothing is wrong. New sessions populate automatically; to import
        history run
        <code>node adapters/claude-code/scripts/backfill-tokens.mjs --include-untracked</code>
      </div>
    </div>
  {:else if view}
    <div class="head">
      <h3 class="title">Token usage</h3>
      <div class="summary">
        <span class="strong">{fmt(view.totals.total)}</span> tokens
        · {view.call_count} calls
        {#if view.model}· {view.model}{/if}
        · {(view.totals.cache_hit_rate * 100).toFixed(0)}% cache hit
      </div>
    </div>

    <div class="bar">
      {#each segments as s}
        {#if s.value > 0}
          <span
            style:width={`${(s.value / totalForBar) * 100}%`}
            style:background={s.color}
            title={`${s.key}: ${fmt(s.value)}`}
          ></span>
        {/if}
      {/each}
    </div>
    <div class="legend">
      {#each segments as s}
        {#if s.value > 0}
          <span class="legend-item">
            <span class="swatch" style:background={s.color}></span>
            <span>{s.key.replace(/_/g, ' ')}</span>
            <span class="legend-value">{fmt(s.value)}</span>
          </span>
        {/if}
      {/each}
    </div>

    {#if view.patterns.length > 0}
      <div class="patterns">
        {#each view.patterns as p}
          {@const Icon = iconFor(p.kind)}
          <div class={`pattern kind-${p.kind}`} title={p.body}>
            <Icon size={12} strokeWidth={1.8} />
            <span>{p.title}</span>
          </div>
        {/each}
      </div>
    {/if}

    <button class="expand" type="button" onclick={() => (expanded = !expanded)}>
      {#if expanded}
        <ChevronDown size={14} strokeWidth={1.6} />
        Hide per-call breakdown
      {:else}
        <ChevronRight size={14} strokeWidth={1.6} />
        Show per-call breakdown ({view.call_count})
      {/if}
    </button>

    {#if expanded}
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th class="num">#</th>
              <th>Time</th>
              <th>Prompt</th>
              <th>Model</th>
              <th class="r">Input</th>
              <th class="r">Output</th>
              <th class="r">Total</th>
              <th>Tools</th>
            </tr>
          </thead>
          <tbody>
            {#each view.calls as c, i}
              <tr>
                <td class="num">{i + 1}</td>
                <td class="ts">{shortTime(c.timestamp)}</td>
                <td class="prompt" title={c.prompt ?? ''}>
                  {c.prompt ?? '(continuation)'}
                </td>
                <td class="model">{c.model}</td>
                <td class="r num">{fmt(c.input_tokens)}</td>
                <td class="r num">{fmt(c.output_tokens)}</td>
                <td class="r num strong">{fmt(c.total_tokens)}</td>
                <td class="tools">
                  {#if c.tools.length > 0}
                    {c.tools.slice(0, 3).join(', ')}{c.tools.length > 3
                      ? ` +${c.tools.length - 3}`
                      : ''}
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</section>

<style>
  .panel {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    padding: 14px;
    margin-bottom: 18px;
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .title {
    margin: 0;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-faint);
  }
  .summary {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--ink-muted);
  }
  .strong {
    color: var(--ink);
    font-weight: 600;
  }
  .bar {
    display: flex;
    width: 100%;
    height: 12px;
    background: var(--bg);
    border-radius: 3px;
    overflow: hidden;
    margin-bottom: 8px;
  }
  .bar span {
    display: block;
    height: 100%;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    font-size: 11px;
    color: var(--ink-muted);
    font-family: var(--font-mono);
    margin-bottom: 10px;
  }
  .legend-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }
  .legend-value {
    color: var(--ink);
  }
  .patterns {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 10px;
  }
  .pattern {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    padding: 3px 8px;
    border-radius: 999px;
    border: 1px solid var(--line);
    background: var(--bg);
    color: var(--ink-muted);
    cursor: help;
  }
  .pattern.kind-warning {
    border-color: var(--c-bug, #c4453d);
    color: var(--c-bug, #c4453d);
  }
  .pattern.kind-info {
    border-color: var(--c-obs, #7b9bd4);
    color: var(--c-obs, #7b9bd4);
  }
  .expand {
    background: transparent;
    border: none;
    color: var(--ink-muted);
    padding: 4px 0;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    cursor: pointer;
  }
  .expand:hover {
    color: var(--ink);
  }
  .table-wrap {
    margin-top: 10px;
    overflow-x: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 11px;
  }
  thead th {
    text-align: left;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-faint);
    padding: 4px 6px;
    border-bottom: 1px solid var(--line);
  }
  tbody td {
    padding: 4px 6px;
    border-bottom: 1px solid var(--line);
    vertical-align: top;
  }
  .num,
  .ts {
    font-family: var(--font-mono);
    color: var(--ink-muted);
  }
  .r {
    text-align: right;
  }
  .prompt {
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink);
  }
  .model,
  .tools {
    font-family: var(--font-mono);
    color: var(--ink-muted);
  }
  .placeholder {
    padding: 12px;
    color: var(--ink-faint);
    font-size: 12px;
  }
  .placeholder.error {
    color: var(--c-bug, #c4453d);
  }
  .placeholder code {
    display: inline-block;
    background: var(--bg);
    padding: 2px 6px;
    border-radius: 3px;
    font-family: var(--font-mono);
    margin-top: 4px;
  }
  .hint {
    margin-top: 4px;
    font-size: 11px;
  }
</style>
