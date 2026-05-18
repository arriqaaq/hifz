<script lang="ts">
  import { page } from '$app/state';
  import {
    Home,
    Layers,
    Workflow,
    Activity,
    Brain,
    Network,
    Boxes,
    GitCommit,
    Coins,
    History,
    HelpCircle,
  } from 'lucide-svelte';

  type Item = {
    icon: typeof Home;
    href: string;
    label: string;
    match: (path: string) => boolean;
  };

  const top: Item[] = [
    { icon: Home, href: '/', label: 'Home', match: (p) => p === '/' },
    { icon: Layers, href: '/sessions', label: 'Sessions', match: (p) => p.startsWith('/sessions') },
    { icon: Workflow, href: '/runs', label: 'Runs', match: (p) => p.startsWith('/runs') },
    { icon: Activity, href: '/observations', label: 'Observations', match: (p) => p.startsWith('/observations') },
    { icon: Brain, href: '/memories', label: 'Memories', match: (p) => p.startsWith('/memories') },
    { icon: Network, href: '/graph', label: 'Graph', match: (p) => p.startsWith('/graph') },
    { icon: Boxes, href: '/atlas', label: 'Atlas', match: (p) => p.startsWith('/atlas') },
    { icon: GitCommit, href: '/commits', label: 'Commits', match: (p) => p.startsWith('/commits') },
    { icon: History, href: '/replay', label: 'Replay', match: (p) => p.startsWith('/replay') },
    { icon: Coins, href: '/tokens', label: 'Tokens', match: (p) => p.startsWith('/tokens') },
  ];

  const bottom: Item[] = [
    { icon: HelpCircle, href: 'https://github.com/arriqaaq/hifz', label: 'Help', match: () => false },
  ];

  let path = $derived(page.url.pathname);
</script>

<nav class="rail" aria-label="Primary">
  <div class="logo" title="hifz">h</div>

  <div class="group">
    {#each top as item}
      {@const Icon = item.icon}
      <a
        href={item.href}
        class="rail-btn"
        class:active={item.match(path)}
        title={item.label}
        aria-label={item.label}
      >
        <Icon size={18} strokeWidth={1.6} />
      </a>
    {/each}
  </div>

  <div class="group bottom">
    {#each bottom as item}
      {@const Icon = item.icon}
      <a
        href={item.href}
        class="rail-btn"
        class:active={item.match(path)}
        title={item.label}
        aria-label={item.label}
        target={item.href.startsWith('http') ? '_blank' : undefined}
        rel={item.href.startsWith('http') ? 'noreferrer' : undefined}
      >
        <Icon size={18} strokeWidth={1.6} />
      </a>
    {/each}
  </div>
</nav>

<style>
  .rail {
    grid-area: rail;
    width: var(--rail-w);
    height: 100vh;
    background: var(--surface);
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 8px 0;
    position: sticky;
    top: 0;
    z-index: 10;
  }

  .logo {
    width: 32px;
    height: 32px;
    border-radius: var(--radius-sm);
    background: var(--ink);
    color: #fff;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: var(--font-display);
    font-weight: 700;
    font-size: 16px;
    margin-bottom: 12px;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 100%;
    align-items: center;
  }

  .group.bottom {
    margin-top: auto;
  }

  .rail-btn {
    width: 32px;
    height: 32px;
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--ink-muted);
    transition: background 120ms, color 120ms;
    position: relative;
  }

  .rail-btn:hover {
    background: var(--surface-alt);
    color: var(--ink);
  }

  .rail-btn.active {
    background: var(--neon);
    color: var(--ink);
  }

  .rail-btn.active::before {
    content: '';
    position: absolute;
    left: -8px;
    top: 6px;
    bottom: 6px;
    width: 2px;
    border-radius: 2px;
    background: var(--ink);
  }
</style>
