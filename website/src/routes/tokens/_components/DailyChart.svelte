<script lang="ts">
  import type { DailyBucket } from '$lib/api';

  type Props = { daily: DailyBucket[] };
  let { daily }: Props = $props();

  // Stack order: input first (bottom), then any breakdown keys, then output (top).
  // Colors are CSS variables so theme inheritance just works.
  const COLORS = [
    'var(--c-mem, #4a8c5b)',     // input
    'var(--c-run, #d4a155)',     // cache_read
    'var(--c-obs, #7b9bd4)',     // cache_creation
    'var(--c-fix, #b478b8)',     // output
    '#888',                       // fallback
  ];

  let breakdownKeys = $derived.by(() => {
    const set = new Set<string>();
    for (const d of daily) {
      for (const k of Object.keys(d.breakdown ?? {})) set.add(k);
    }
    return Array.from(set).sort();
  });

  type Series = { key: string; label: string; color: string; values: number[] };

  let series = $derived.by(() => {
    const out: Series[] = [];
    out.push({
      key: 'input',
      label: 'input',
      color: COLORS[0],
      values: daily.map((d) => d.input),
    });
    breakdownKeys.forEach((k, i) => {
      out.push({
        key: `breakdown.${k}`,
        label: k.replace(/_/g, ' '),
        color: COLORS[(i + 1) % COLORS.length],
        values: daily.map((d) => d.breakdown?.[k] ?? 0),
      });
    });
    out.push({
      key: 'output',
      label: 'output',
      color: COLORS[3],
      values: daily.map((d) => d.output),
    });
    return out;
  });

  let dayTotals = $derived(daily.map((d) => d.total));
  let maxTotal = $derived(Math.max(1, ...dayTotals));

  const W = 720;
  const H = 220;
  const PAD_L = 36;
  const PAD_R = 8;
  const PAD_T = 8;
  const PAD_B = 22;

  let chartW = $derived(W - PAD_L - PAD_R);
  let chartH = $derived(H - PAD_T - PAD_B);

  let barW = $derived(daily.length === 0 ? 0 : Math.max(2, (chartW - daily.length * 2) / Math.max(1, daily.length)));

  function fmt(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${Math.round(n / 1_000)}K`;
    return String(n);
  }

  function shortDate(d: string): string {
    return d.slice(5); // MM-DD
  }
</script>

{#if daily.length === 0}
  <div class="empty">No data in this range.</div>
{:else}
  <svg viewBox={`0 0 ${W} ${H}`} class="chart" role="img" aria-label="Daily token usage">
    <!-- y-axis ticks -->
    {#each [0, 0.5, 1] as t}
      <line
        x1={PAD_L}
        x2={W - PAD_R}
        y1={PAD_T + chartH * (1 - t)}
        y2={PAD_T + chartH * (1 - t)}
        stroke="var(--line)"
        stroke-width="0.5"
      />
      <text
        x={PAD_L - 6}
        y={PAD_T + chartH * (1 - t) + 3}
        text-anchor="end"
        font-size="9"
        font-family="var(--font-mono)"
        fill="var(--ink-faint)"
      >
        {fmt(maxTotal * t)}
      </text>
    {/each}

    <!-- bars (one per day, stacked) -->
    {#each daily as d, i}
      {@const x = PAD_L + i * (barW + 2)}
      {#each series as s, si}
        {@const stackTop = series.slice(0, si).reduce((sum, ss) => sum + ss.values[i], 0)}
        {@const value = s.values[i]}
        {@const y = PAD_T + chartH * (1 - (stackTop + value) / maxTotal)}
        {@const h = chartH * (value / maxTotal)}
        {#if value > 0}
          <rect x={x} y={y} width={barW} height={h} fill={s.color}>
            <title>{d.date} · {s.label}: {fmt(value)}</title>
          </rect>
        {/if}
      {/each}
    {/each}

    <!-- x-axis labels (sparse: every Nth date) -->
    {#each daily as d, i}
      {#if daily.length <= 14 || i % Math.ceil(daily.length / 8) === 0}
        <text
          x={PAD_L + i * (barW + 2) + barW / 2}
          y={H - 6}
          text-anchor="middle"
          font-size="9"
          font-family="var(--font-mono)"
          fill="var(--ink-faint)"
        >
          {shortDate(d.date)}
        </text>
      {/if}
    {/each}
  </svg>

  <div class="legend">
    {#each series as s}
      <span class="legend-item"><span class="swatch" style:background={s.color}></span>{s.label}</span>
    {/each}
  </div>
{/if}

<style>
  .chart {
    width: 100%;
    height: auto;
    max-height: 240px;
    display: block;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    margin-top: 8px;
    font-size: 11px;
    color: var(--ink-muted);
    font-family: var(--font-mono);
  }
  .legend-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }
  .empty {
    padding: 24px;
    text-align: center;
    color: var(--ink-faint);
    font-size: 12px;
  }
</style>
