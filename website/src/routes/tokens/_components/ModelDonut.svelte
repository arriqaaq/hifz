<script lang="ts">
  import type { ModelBucket } from '$lib/api';

  type Props = { models: ModelBucket[] };
  let { models }: Props = $props();

  const COLORS = ['var(--c-mem, #4a8c5b)', 'var(--c-run, #d4a155)', 'var(--c-obs, #7b9bd4)', 'var(--c-fix, #b478b8)', '#888', '#aaa'];

  let total = $derived(models.reduce((s, m) => s + m.total, 0));

  type Slice = { model: string; total: number; pct: number; from: number; to: number; color: string };

  let slices = $derived.by(() => {
    if (total <= 0) return [] as Slice[];
    let acc = 0;
    return models.map((m, i) => {
      const pct = m.total / total;
      const from = acc;
      acc += pct;
      return { model: m.model, total: m.total, pct, from, to: acc, color: COLORS[i % COLORS.length] };
    });
  });

  // Convert fraction (0-1) to (x, y) on the unit circle, rotated so 0 = top.
  function polar(frac: number, r: number) {
    const a = frac * Math.PI * 2 - Math.PI / 2;
    return [Math.cos(a) * r, Math.sin(a) * r];
  }

  function arcPath(from: number, to: number, rOuter: number, rInner: number): string {
    const [x1, y1] = polar(from, rOuter);
    const [x2, y2] = polar(to, rOuter);
    const [x3, y3] = polar(to, rInner);
    const [x4, y4] = polar(from, rInner);
    const large = to - from > 0.5 ? 1 : 0;
    return `M ${x1} ${y1} A ${rOuter} ${rOuter} 0 ${large} 1 ${x2} ${y2} L ${x3} ${y3} A ${rInner} ${rInner} 0 ${large} 0 ${x4} ${y4} Z`;
  }

  function fmt(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${Math.round(n / 1_000)}K`;
    return String(n);
  }
</script>

{#if slices.length === 0}
  <div class="empty">No data.</div>
{:else}
  <div class="row">
    <svg viewBox="-60 -60 120 120" class="donut" role="img" aria-label="Tokens by model">
      {#each slices as s}
        <path d={arcPath(s.from, s.to, 50, 30)} fill={s.color}>
          <title>{s.model}: {fmt(s.total)}</title>
        </path>
      {/each}
    </svg>
    <ul class="legend">
      {#each slices as s}
        <li>
          <span class="swatch" style:background={s.color}></span>
          <span class="model" title={s.model}>{s.model}</span>
          <span class="value">{fmt(s.total)}</span>
        </li>
      {/each}
    </ul>
  </div>
{/if}

<style>
  .row {
    display: flex;
    gap: 12px;
    align-items: center;
  }
  .donut {
    width: 120px;
    height: 120px;
    flex-shrink: 0;
  }
  .legend {
    list-style: none;
    margin: 0;
    padding: 0;
    flex: 1;
    min-width: 0;
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--ink-muted);
  }
  .legend li {
    display: grid;
    grid-template-columns: 12px 1fr auto;
    align-items: center;
    gap: 8px;
    margin-bottom: 4px;
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }
  .model {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .value {
    color: var(--ink);
  }
  .empty {
    padding: 24px;
    text-align: center;
    color: var(--ink-faint);
    font-size: 12px;
  }
</style>
