<!-- SPDX-License-Identifier: Apache-2.0 -->
<!-- Inspect view: a memory's header + its lineage/links/evolution rows,
     reusing DeltaView for the rows so it matches the live diff exactly. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { MemoryView, RenderTokens, Span } from '$lib/types';
  import { loadTokens, toneColor } from '$lib/tokens';
  import DeltaView from './DeltaView.svelte';

  let {
    view,
    tokens = null,
  }: { view: MemoryView; tokens?: RenderTokens | null } = $props();

  let loaded = $state<RenderTokens | null>(null);
  let tk = $derived(tokens ?? loaded);
  onMount(async () => {
    if (!tokens) loaded = await loadTokens();
  });

  function spanCss(s: Span): string {
    let css = `color:${toneColor(tk, s.style.tone)};`;
    if (s.style.bold) css += 'font-weight:600;';
    if (s.style.dim) css += 'opacity:.6;';
    if (s.style.strike) css += 'text-decoration:line-through;';
    return css;
  }
</script>

<div class="mv">
  <div class="hdr">
    {#each view.header as s}
      <span style={spanCss(s)}>{s.text}</span>
    {/each}
  </div>
  <DeltaView lines={view.rows} tokens={tk} />
</div>

<style>
  .mv {
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 12px 14px;
  }
  .hdr {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5em;
    align-items: baseline;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 14px;
    margin-bottom: 8px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--line);
  }
</style>
