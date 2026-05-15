<script lang="ts">
  import type { UsageTotals } from '$lib/api';

  type Props = {
    totals: UsageTotals;
    callCount: number;
    sessionCount: number;
    dateRange: { from: string; to: string } | null;
  };
  let { totals, callCount, sessionCount, dateRange }: Props = $props();

  function fmtTokens(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 10_000) return `${Math.round(n / 1_000)}K`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
    return n.toLocaleString();
  }

  function fmtUsd(n: number): string {
    if (n >= 100) return `$${n.toFixed(0)}`;
    if (n >= 1) return `$${n.toFixed(2)}`;
    return `$${n.toFixed(4)}`;
  }
</script>

<div class="grid">
  <div class="stat">
    <div class="value">{fmtUsd(totals.cost_usd)}</div>
    <div class="label">cost (billable)</div>
  </div>
  <div class="stat">
    <div class="value">{callCount.toLocaleString()}</div>
    <div class="label">inference calls</div>
  </div>
  <div class="stat">
    <div class="value">{sessionCount.toLocaleString()}</div>
    <div class="label">sessions</div>
  </div>
  <div class="stat">
    <div class="value">{(totals.cache_hit_rate * 100).toFixed(1)}%</div>
    <div class="label">cache hit rate</div>
  </div>
</div>

<div class="breakdown">
  <div class="bd-row"><span class="bd-label">input</span><span class="bd-val">{fmtTokens(totals.input)}</span></div>
  <div class="bd-row"><span class="bd-label">output</span><span class="bd-val">{fmtTokens(totals.output)}</span></div>
  <div class="bd-row"><span class="bd-label">cache create</span><span class="bd-val">{fmtTokens(totals.cache_creation)}</span></div>
  <div class="bd-row"><span class="bd-label">cache read</span><span class="bd-val">{fmtTokens(totals.cache_read)}</span></div>
  <div class="bd-row total"><span class="bd-label">raw token volume</span><span class="bd-val">{fmtTokens(totals.total)}</span></div>
</div>

{#if dateRange}
  <div class="range">{dateRange.from} → {dateRange.to}</div>
{/if}

{#if totals.cost_unknown_calls > 0 || totals.aux_calls > 0}
  <div class="note">
    {#if totals.cost_unknown_calls > 0}
      {totals.cost_unknown_calls.toLocaleString()} call{totals.cost_unknown_calls === 1 ? '' : 's'} with unknown model — cost not included.
    {/if}
    {#if totals.aux_calls > 0}
      {totals.aux_calls.toLocaleString()} untracked auxiliary call{totals.aux_calls === 1 ? '' : 's'} (title generation, summarization).
    {/if}
  </div>
{/if}

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 12px;
    margin-bottom: 8px;
  }
  .stat {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    padding: 14px;
    text-align: left;
  }
  .value {
    font-family: var(--font-mono);
    font-size: 22px;
    font-weight: 600;
    color: var(--ink);
    line-height: 1.1;
  }
  .label {
    margin-top: 4px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-faint);
  }
  .breakdown {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 18px;
    padding: 10px 14px;
    margin-bottom: 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--surface);
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .bd-row {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
  }
  .bd-label {
    color: var(--ink-faint);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 10px;
  }
  .bd-val {
    color: var(--ink);
  }
  .bd-row.total .bd-val {
    color: var(--ink-faint);
  }
  .range {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-faint);
    margin-bottom: 6px;
  }
  .note {
    font-size: 11px;
    color: var(--ink-faint);
    margin-bottom: 18px;
  }
</style>
