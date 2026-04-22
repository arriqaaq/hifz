<script lang="ts">
  import '../app.css';
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import { getHealth } from '$lib/api';
  import type { HealthResponse } from '$lib/types';

  let { children } = $props();

  let health = $state<HealthResponse | null>(null);

  const tabs = [
    { label: 'Dashboard', href: '/', desc: 'Overview of sessions, commits, and top concepts' },
    { label: 'Sessions', href: '/sessions', desc: 'Claude Code sessions — each conversation from start to end' },
    { label: 'Runs', href: '/runs', desc: 'Task-scoped trajectories within a session (prompt to completion)' },
    { label: 'Observations', href: '/observations', desc: 'Raw events: tool calls, prompts, and outputs' },
    { label: 'Memories', href: '/memories', desc: 'Curated knowledge: patterns, preferences, and facts' },
    { label: 'Graph', href: '/graph', desc: 'Visual map of concepts and their connections' },
  ];

  function isActive(href: string): boolean {
    if (href === '/') return page.url.pathname === '/';
    return page.url.pathname.startsWith(href);
  }

  function formatDate(): string {
    return new Date().toLocaleDateString('en-US', {
      weekday: 'short',
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    }).toUpperCase();
  }

  onMount(() => {
    getHealth().then(h => { health = h; }).catch(() => {});
  });
</script>

<svelte:head>
  <title>hifz</title>
  <meta name="viewport" content="width=device-width, initial-scale=1" />
</svelte:head>

<div class="app">
  <header class="app-header">
    <div class="header-left">
      <span class="logo">hifz</span>
      {#if health}
        <span class="version">v{health.version}</span>
      {/if}
    </div>
    <div class="header-right">
      <span class="date">{formatDate()}</span>
      {#if health}
        <span class="ws-status connected">
          <span class="health-dot healthy"></span>
          {health.status}
        </span>
      {:else}
        <span class="ws-status">
          <span class="health-dot"></span>
          disconnected
        </span>
      {/if}
    </div>
  </header>

  <nav class="tab-bar">
    {#each tabs as tab}
      <a
        href={tab.href}
        class="tab"
        class:active={isActive(tab.href)}
        title={tab.desc}
      >{tab.label}</a>
    {/each}
  </nav>

  <main class="content">
    {@render children()}
  </main>
</div>

<style>
  .app {
    min-height: 100vh;
    background: var(--bg);
  }

  .app-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 40px;
    border-bottom: 1px solid var(--border-light);
    background: var(--bg);
  }

  .header-left {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .logo {
    font-family: var(--font-display);
    font-size: 22px;
    font-weight: 900;
    color: var(--ink);
  }

  .version {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
  }

  .header-right {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .date {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
    letter-spacing: 0.06em;
  }

  .ws-status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 1px solid var(--border-light);
    font-size: 10px;
    font-family: var(--font-ui);
    padding: 3px 10px;
    text-transform: uppercase;
    font-weight: 600;
    letter-spacing: 0.08em;
    color: var(--ink-faint);
  }
  .ws-status.connected {
    border-color: var(--green);
    color: var(--green);
  }

  .tab-bar {
    display: flex;
    gap: 0;
    border-bottom: 1px solid var(--border-light);
    padding: 0 40px;
    background: var(--bg);
  }

  .tab {
    padding: 12px 16px;
    font-family: var(--font-ui);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--ink-muted);
    border-bottom: 2px solid transparent;
    transition: color 150ms, border-color 150ms;
    text-decoration: none;
  }
  .tab:hover {
    color: var(--ink);
  }
  .tab.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .content {
    max-width: 1400px;
    margin: 0 auto;
    padding: 24px 40px;
    background: var(--bg);
  }
</style>
