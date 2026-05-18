<!-- SPDX-License-Identifier: Apache-2.0 -->
<!--
  The one place delta markup lives. Renders a structured MemoryDelta
  (lines of glyph + styled spans) identically for live results and replay.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import type { DeltaLine, Glyph, RenderTokens, Span } from '$lib/types';
  import { loadTokens, toneColor, opTone } from '$lib/tokens';

  let {
    lines,
    tokens = null,
  }: { lines: DeltaLine[]; tokens?: RenderTokens | null } = $props();

  let loaded = $state<RenderTokens | null>(null);
  let tk = $derived(tokens ?? loaded);
  onMount(async () => {
    if (!tokens) loaded = await loadTokens();
  });

  const GLYPH: Record<Glyph, string> = {
    plus: '+',
    tilde: '~',
    slashed: '⊘',
    arrow: '→',
    recycle: '↻',
    cross: '×',
    bang: '!',
  };

  function spanCss(s: Span): string {
    let css = `color:${toneColor(tk, s.style.tone)};`;
    if (s.style.bold) css += 'font-weight:600;';
    if (s.style.dim) css += 'opacity:.6;';
    if (s.style.strike) css += 'text-decoration:line-through;';
    return css;
  }

  function citeLabel(c: NonNullable<Span['cite']>): string {
    if (c.kind === 'memory') return c.id;
    if (c.kind === 'edge') return `${c.relation}:${c.target}`;
    return c.id;
  }
  function citeHref(c: NonNullable<Span['cite']>): string | undefined {
    if (c.kind === 'memory') return `/memories/${encodeURIComponent(c.id)}`;
    if (c.kind === 'edge') return `/memories/${encodeURIComponent(c.target)}`;
    return undefined;
  }
</script>

<div class="delta">
  {#each lines as line}
    <div class="line">
      <span class="glyph" style={`color:${toneColor(tk, opTone(line.op))}`}>
        {GLYPH[line.glyph] ?? '·'}
      </span>
      {#each line.spans as s}
        <span style={spanCss(s)}>{s.text}</span>
        {#if s.cite}
          {#if citeHref(s.cite)}
            <a class="cite" href={citeHref(s.cite)}>{citeLabel(s.cite)}</a>
          {:else}
            <span class="cite">{citeLabel(s.cite)}</span>
          {/if}
        {/if}
      {/each}
    </div>
  {/each}
  {#if lines.length === 0}
    <div class="empty">no changes</div>
  {/if}
</div>

<style>
  .delta {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 13px;
    line-height: 1.6;
  }
  .line {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4em;
    align-items: baseline;
  }
  .glyph {
    width: 1ch;
    font-weight: 700;
    flex: none;
  }
  .cite {
    font-size: 11px;
    opacity: 0.7;
    border: 1px solid currentColor;
    border-radius: 4px;
    padding: 0 4px;
    text-decoration: none;
  }
  a.cite:hover {
    opacity: 1;
  }
  .empty {
    opacity: 0.5;
    font-style: italic;
  }
</style>
