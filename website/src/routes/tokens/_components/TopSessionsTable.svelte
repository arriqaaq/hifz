<script lang="ts">
  import type { UsageSessionRow } from '$lib/api';

  type Props = { rows: UsageSessionRow[] };
  let { rows }: Props = $props();

  function fmt(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 10_000) return `${Math.round(n / 1_000)}K`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
    return String(n);
  }
</script>

{#if rows.length === 0}
  <div class="empty">No sessions in this range.</div>
{:else}
  <table>
    <thead>
      <tr>
        <th class="num">#</th>
        <th>First prompt</th>
        <th>Tokens</th>
        <th>Calls</th>
        <th>Model</th>
        <th>Date</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as r, i}
        <tr>
          <td class="num">{i + 1}</td>
          <td class="prompt" title={r.first_prompt ?? ''}>
            <a href={`/sessions/${encodeURIComponent(r.session_id)}`}>
              {r.first_prompt ?? '(no prompt)'}
            </a>
          </td>
          <td class="value">{fmt(r.total)}</td>
          <td class="num right">{r.calls}</td>
          <td class="model">{r.model}</td>
          <td class="date">{r.date}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  thead th {
    text-align: left;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-faint);
    padding: 6px 8px;
    border-bottom: 1px solid var(--line);
  }
  tbody td {
    padding: 6px 8px;
    border-bottom: 1px solid var(--line);
  }
  .num {
    width: 36px;
    color: var(--ink-faint);
    font-family: var(--font-mono);
  }
  .num.right {
    text-align: right;
  }
  .value {
    font-family: var(--font-mono);
    color: var(--ink);
    text-align: right;
    width: 80px;
  }
  .prompt {
    max-width: 420px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .prompt a {
    color: var(--ink);
    text-decoration: none;
  }
  .prompt a:hover {
    text-decoration: underline;
  }
  .model,
  .date {
    font-family: var(--font-mono);
    color: var(--ink-muted);
    font-size: 11px;
  }
  .empty {
    padding: 18px;
    text-align: center;
    color: var(--ink-faint);
    font-size: 12px;
  }
</style>
