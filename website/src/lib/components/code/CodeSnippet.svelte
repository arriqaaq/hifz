<script lang="ts">
  import { tokenize } from '$lib/code/highlight';

  let { code, language = '' }: { code: string; language?: string } = $props();

  let tokens = $derived(tokenize(code ?? '', language));
</script>

<pre class="snippet"><code>{#each tokens as t}<span class={t.cls}>{t.text}</span>{/each}</code></pre>

<style>
  .snippet {
    margin: 0;
    padding: 10px 12px;
    background: var(--surface-alt);
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.55;
    overflow-x: auto;
    white-space: pre;
    color: var(--ink-secondary);
  }
  code {
    font-family: inherit;
  }
  /* Token colors — identical to docs/img/ui-code-search.svg (.kw/.fn/.str/.cmt). */
  .tok-kw   { color: #9b59b6; }
  .tok-fn   { color: #2563eb; }
  .tok-str  { color: #15803d; }
  .tok-cmt  { color: #9a9a91; font-style: italic; }
  .tok-num  { color: #b45309; }
  .tok-type { color: #0e7490; }
</style>
