<script lang="ts">
  import { page } from '$app/state';
  import { ChevronRight } from 'lucide-svelte';

  type Crumb = { label: string; href: string | null; chip?: 'session' | 'run' | 'commit' };

  function shortId(id: string): string {
    return id.length > 10 ? id.slice(0, 8) + '…' : id;
  }

  let crumbs = $derived.by<Crumb[]>(() => {
    const path: string = page.url.pathname;
    const params = page.params as Record<string, string>;
    const out: Crumb[] = [{ label: 'Home', href: '/' }];

    if (path === '/') return [{ label: 'Home', href: null }];
    if (path === '/sessions') return [...out, { label: 'Sessions', href: null }];
    if (path.startsWith('/sessions/') && params.id) {
      return [
        ...out,
        { label: 'Sessions', href: '/sessions' },
        { label: shortId(params.id), href: null, chip: 'session' },
      ];
    }
    if (path === '/runs') return [...out, { label: 'Runs', href: null }];
    if (path.startsWith('/runs/') && params.id) {
      return [
        ...out,
        { label: 'Runs', href: '/runs' },
        { label: shortId(params.id), href: null, chip: 'run' },
      ];
    }
    if (path === '/observations') return [...out, { label: 'Observations', href: null }];
    if (path === '/memories') return [...out, { label: 'Memories', href: null }];
    if (path === '/code') return [...out, { label: 'Code search', href: null }];
    if (path === '/graph') return [...out, { label: 'Graph', href: null }];
    if (path === '/commits') return [...out, { label: 'Commits', href: null }];
    if (path.startsWith('/commits/') && params.sha) {
      return [
        ...out,
        { label: 'Commits', href: '/commits' },
        { label: shortId(params.sha), href: null, chip: 'commit' },
      ];
    }
    return out;
  });

  function chipColor(c?: Crumb['chip']): string {
    if (c === 'session') return 'var(--c-session)';
    if (c === 'run') return 'var(--c-run)';
    if (c === 'commit') return 'var(--c-commit)';
    return 'var(--ink)';
  }
</script>

<nav class="breadcrumb" aria-label="Breadcrumb">
  {#each crumbs as c, i}
    {#if i > 0}
      <ChevronRight size={12} strokeWidth={1.5} class="sep" />
    {/if}
    {#if c.href}
      <a href={c.href} class="crumb">{c.label}</a>
    {:else if c.chip}
      <span class="crumb crumb-chip" style={`--chip: ${chipColor(c.chip)}`}>{c.label}</span>
    {:else}
      <span class="crumb crumb-current">{c.label}</span>
    {/if}
  {/each}
</nav>

<style>
  .breadcrumb {
    display: flex;
    align-items: center;
    gap: 6px;
    font-family: var(--font-ui);
    font-size: 12px;
    color: var(--ink-muted);
    min-width: 0;
  }

  .crumb {
    color: var(--ink-muted);
    padding: 2px 4px;
    border-radius: 4px;
    transition: color 120ms, background 120ms;
  }

  a.crumb:hover {
    color: var(--ink);
    background: var(--surface-alt);
  }

  .crumb-current {
    color: var(--ink);
    font-weight: 500;
  }

  .crumb-chip {
    color: var(--chip);
    background: color-mix(in srgb, var(--chip) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--chip) 25%, transparent);
    padding: 1px 8px;
    border-radius: 999px;
    font-family: var(--font-mono);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  :global(.breadcrumb .sep) {
    color: var(--ink-faint);
    flex-shrink: 0;
  }
</style>
