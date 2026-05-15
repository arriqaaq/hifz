<script lang="ts">
  import type { PromptRow } from '$lib/api';

  type Props = { rows: PromptRow[] };
  let { rows }: Props = $props();

  function fmt(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 10_000) return `${Math.round(n / 1_000)}K`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
    return String(n);
  }

  let maxTotal = $derived(Math.max(1, ...rows.map((r) => r.total)));

  function segments(r: PromptRow) {
    const out: { key: string; v: number; color: string }[] = [];
    out.push({ key: 'input', v: r.input, color: 'var(--c-mem, #4a8c5b)' });
    const keys = Object.keys(r.breakdown ?? {}).sort();
    const palette = ['var(--c-run, #d4a155)', 'var(--c-obs, #7b9bd4)', '#888'];
    keys.forEach((k, i) => out.push({ key: k, v: r.breakdown[k] ?? 0, color: palette[i % palette.length] }));
    out.push({ key: 'output', v: r.output, color: 'var(--c-fix, #b478b8)' });
    return out;
  }
</script>

{#if rows.length === 0}
  <div class="empty">No prompts in this range.</div>
{:else}
  <table>
    <thead>
      <tr>
        <th class="num">#</th>
        <th>Prompt</th>
        <th>Tokens</th>
        <th>Bar</th>
        <th>Session</th>
        <th>Model</th>
        <th>Date</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as r, i}
        <tr>
          <td class="num">{i + 1}</td>
          <td class="prompt" title={r.prompt}>{r.prompt}</td>
          <td class="value">{fmt(r.total)}</td>
          <td class="bar-cell">
            <span class="bar">
              {#each segments(r) as seg}
                {#if seg.v > 0}
                  <span style:width={`${(seg.v / maxTotal) * 100}%`} style:background={seg.color} title={`${seg.key}: ${fmt(seg.v)}`}></span>
                {/if}
              {/each}
            </span>
          </td>
          <td class="sid"><a href={`/sessions/${encodeURIComponent(r.session_id)}`}>{r.session_id.slice(0, 8)}</a></td>
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
    vertical-align: middle;
  }
  .num {
    width: 32px;
    color: var(--ink-faint);
    font-family: var(--font-mono);
  }
  .value {
    font-family: var(--font-mono);
    color: var(--ink);
    text-align: right;
    width: 80px;
  }
  .prompt {
    max-width: 340px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink);
  }
  .bar-cell {
    width: 200px;
  }
  .bar {
    display: flex;
    width: 100%;
    height: 10px;
    background: var(--bg);
    border-radius: 2px;
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
  }
  .sid,
  .model,
  .date {
    font-family: var(--font-mono);
    color: var(--ink-muted);
    font-size: 11px;
  }
  .sid a {
    color: inherit;
    text-decoration: none;
  }
  .sid a:hover {
    color: var(--ink);
  }
  .empty {
    padding: 18px;
    text-align: center;
    color: var(--ink-faint);
    font-size: 12px;
  }
</style>
