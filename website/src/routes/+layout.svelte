<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { getHealth } from '$lib/api';
  import type { HealthResponse } from '$lib/types';
  import Sidebar from '$lib/components/shell/Sidebar.svelte';
  import SecondaryPanel from '$lib/components/shell/SecondaryPanel.svelte';
  import Breadcrumb from '$lib/components/shell/Breadcrumb.svelte';
  import CommandBar from '$lib/components/shell/CommandBar.svelte';
  import DetailDrawer from '$lib/components/common/DetailDrawer.svelte';
  import { shell } from '$lib/stores/shell.svelte';
  import { PanelLeft, PanelLeftClose, Command as CommandIcon } from 'lucide-svelte';

  let { children } = $props();

  let health = $state<HealthResponse | null>(null);

  onMount(() => {
    getHealth()
      .then((h) => {
        health = h;
      })
      .catch(() => {});

    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      const inField =
        target &&
        (target.tagName === 'INPUT' ||
          target.tagName === 'TEXTAREA' ||
          target.isContentEditable);

      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        shell.toggleCommand();
        return;
      }
      if (!inField && e.key === '[' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        shell.togglePanel();
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
</script>

<svelte:head>
  <title>hifz</title>
  <meta name="viewport" content="width=device-width, initial-scale=1" />
</svelte:head>

<div class="app" class:panel-collapsed={!shell.panelOpen}>
  <Sidebar />
  <SecondaryPanel />

  <header class="topbar">
    <button
      type="button"
      class="icon-btn"
      onclick={() => shell.togglePanel()}
      title={shell.panelOpen ? 'Collapse panel ([)' : 'Expand panel ([)'}
      aria-label="Toggle secondary panel"
    >
      {#if shell.panelOpen}
        <PanelLeftClose size={16} strokeWidth={1.6} />
      {:else}
        <PanelLeft size={16} strokeWidth={1.6} />
      {/if}
    </button>

    <Breadcrumb />

    <div class="topbar-spacer"></div>

    <button
      type="button"
      class="cmdk-trigger"
      onclick={() => shell.toggleCommand()}
      title="Command palette (⌘K)"
    >
      <CommandIcon size={12} strokeWidth={1.6} />
      <span class="cmdk-label">Search…</span>
      <span class="kbd">⌘K</span>
    </button>

    {#if health}
      <span class="health-pill" title={`Server uptime ${Math.floor(health.uptime_seconds / 60)}m`}>
        <span class="dot dot-ok"></span>
        v{health.version}
      </span>
    {:else}
      <span class="health-pill health-pill--off">
        <span class="dot"></span>
        offline
      </span>
    {/if}
  </header>

  <main class="content">
    {@render children()}
  </main>
</div>

<CommandBar />

{#if shell.drawerOpen && shell.drawerItem}
  {@const item = shell.drawerItem}
  {#if item.kind === 'observation'}
    <DetailDrawer
      item={{ kind: 'observation', data: item.data }}
      onClose={() => shell.closeDrawer()}
      onFilterToSession={item.onFilterToSession}
    />
  {:else if item.kind === 'memory'}
    <DetailDrawer item={{ kind: 'memory', data: item.data }} onClose={() => shell.closeDrawer()} />
  {/if}
{/if}

<style>
  .app {
    display: grid;
    grid-template-columns: var(--rail-w) var(--panel-w) 1fr;
    grid-template-rows: 48px 1fr;
    grid-template-areas:
      'rail panel header'
      'rail panel main';
    min-height: 100vh;
    background: var(--bg);
  }

  .app.panel-collapsed {
    grid-template-columns: var(--rail-w) 0 1fr;
  }

  .topbar {
    grid-area: header;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 16px;
    background: var(--bg);
    border-bottom: 1px solid var(--ink);
    position: sticky;
    top: 0;
    z-index: 5;
  }

  .topbar-spacer {
    flex: 1;
  }

  .icon-btn {
    width: 28px;
    height: 28px;
    border-radius: var(--radius-sm);
    color: var(--ink-muted);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px solid transparent;
    cursor: pointer;
    flex-shrink: 0;
  }
  .icon-btn:hover {
    background: var(--surface-alt);
    color: var(--ink);
  }

  .cmdk-trigger {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 5px 10px 5px 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--ink-muted);
    font-size: 12px;
    font-family: var(--font-ui);
    cursor: pointer;
    min-width: 200px;
  }
  .cmdk-trigger:hover {
    border-color: var(--line-strong);
    color: var(--ink);
  }
  .cmdk-label {
    flex: 1;
    text-align: left;
  }

  .kbd {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 1px 5px;
    background: var(--bg);
  }

  .health-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 10px;
    border: 1px solid var(--line);
    border-radius: 999px;
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--ink-muted);
    background: var(--surface);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ink-faint);
  }
  .dot-ok {
    background: var(--c-run);
  }

  .content {
    grid-area: main;
    padding: 18px 24px;
    overflow-x: hidden;
  }
</style>
