<script lang="ts">
  import { onMount } from 'svelte';
  import { getHealth, getDigest, getSessions, getCommits } from '$lib/api';
  import type { HealthResponse, ProjectDigest, Session, Commit } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import EntityChip from '$lib/components/entity/EntityChip.svelte';
  import { Database, Globe, Zap, Layers, BookOpen, Plug } from 'lucide-svelte';

  let health = $state<HealthResponse | null>(null);
  let digest = $state<ProjectDigest | null>(null);
  let sessions = $state<Session[]>([]);
  let commits = $state<Commit[]>([]);
  let loading = $state(true);
  let error = $state('');

  function extractId(id: unknown): string {
    if (typeof id === 'string') return id;
    if (id && typeof id === 'object') {
      const o = id as Record<string, unknown>;
      if (typeof o.key === 'string') return o.key;
      if (o.key && typeof o.key === 'object' && 'String' in (o.key as Record<string, unknown>)) {
        return (o.key as { String: string }).String;
      }
    }
    return String(id);
  }

  async function refresh() {
    try {
      const [h, d, s, c] = await Promise.all([
        getHealth().catch(() => null),
        getDigest().catch(() => null),
        getSessions(8).catch(() => ({ sessions: [] as Session[] })),
        getCommits(undefined, 5).catch(() => ({ commits: [] as Commit[] })),
      ]);
      health = h && 'status' in h ? h : null;
      digest = d && 'top_keywords' in d ? d : null;
      sessions = s?.sessions ?? [];
      commits = c?.commits ?? [];
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to connect';
    } finally {
      loading = false;
    }
  }

  onMount(refresh);

  function fmtUptime(s: number): string {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (h > 24) return `${Math.floor(h / 24)}d`;
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m`;
  }
</script>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="card" style="border-color: var(--accent);">
    <div class="card-title" style="color: var(--accent);">Connection Error</div>
    <p>{error}</p>
    <p style="font-size: 12px; color: var(--ink-faint); font-family: var(--font-mono);">
      Make sure the hifz server is running on port 3111
    </p>
  </div>
{:else if health && health.observations === 0}
  <div class="welcome">
    <h2 class="welcome-title">Welcome to hifz</h2>
    <p class="welcome-sub">No memories recorded yet. Get started:</p>

    <div class="welcome-grid">
      <a class="welcome-tile" href="https://github.com/arriqaaq/hifz#claude-code" target="_blank" rel="noreferrer">
        <Plug size={18} strokeWidth={1.6} />
        <h4>Connect Claude Code</h4>
        <p>Add the hifz hooks to <code>~/.claude/settings.json</code>.</p>
      </a>
      <a class="welcome-tile" href="https://github.com/arriqaaq/hifz" target="_blank" rel="noreferrer">
        <BookOpen size={18} strokeWidth={1.6} />
        <h4>Read the docs</h4>
        <p>Architecture, MCP tools, and adapters.</p>
      </a>
    </div>
  </div>
{:else if health}
  <h2 class="page-h">Status</h2>
  <div class="status-grid">
    <div class="status-tile">
      <div class="status-head"><Database size={14} strokeWidth={1.6} /> Database</div>
      <div class="status-value">healthy</div>
      <div class="status-sub">{health.observations.toLocaleString()} observations</div>
    </div>
    <div class="status-tile">
      <div class="status-head"><Globe size={14} strokeWidth={1.6} /> REST API</div>
      <div class="status-value">v{health.version}</div>
      <div class="status-sub">uptime {fmtUptime(health.uptime_seconds)}</div>
    </div>
    <div class="status-tile">
      <div class="status-head"><Zap size={14} strokeWidth={1.6} /> Hooks</div>
      <div class="status-value">{sessions.filter((s) => !s.ended_at).length} active</div>
      <div class="status-sub">{health.sessions} sessions total</div>
    </div>
    <div class="status-tile">
      <div class="status-head"><Layers size={14} strokeWidth={1.6} /> Memories</div>
      <div class="status-value">{health.memories}</div>
      <div class="status-sub">long-term store</div>
    </div>
  </div>

  <div class="two-col">
    {#if sessions.length > 0}
      <div class="card">
        <div class="card-title">Recent Sessions</div>
        <table>
          <thead>
            <tr>
              <th>Session</th>
              <th>Project</th>
              <th>Status</th>
              <th>Obs</th>
            </tr>
          </thead>
          <tbody>
            {#each sessions as s}
              <tr>
                <td><EntityChip kind="session" id={extractId(s.id)} size="sm" /></td>
                <td><span class="badge badge-cyan">{s.project ?? '—'}</span></td>
                <td>
                  {#if s.ended_at}
                    <span class="badge badge-blue">completed</span>
                  {:else}
                    <span class="badge badge-green">active</span>
                  {/if}
                </td>
                <td class="mono">{s.observation_count}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    <div class="right-col">
      {#if digest && digest.top_keywords && digest.top_keywords.length > 0}
        <div class="card">
          <div class="card-title">Top Keywords</div>
          {#each digest.top_keywords.slice(0, 8) as c}
            <div class="list-row">
              <span class="list-name">{c.keyword}</span>
              <span class="list-count">{c.frequency}</span>
            </div>
          {/each}
        </div>
      {/if}
      {#if commits.length > 0}
        <div class="card">
          <div class="card-title">Recent Commits</div>
          {#each commits as c}
            {@const sha = c.metadata?.sha ?? ''}
            <div class="list-row">
              <a href="/commits/{sha}" class="list-name mono">{sha.slice(0, 8)}</a>
              <span class="list-count" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:160px;">
                {c.metadata?.message ?? c.title}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .page-h {
    margin: 0 0 16px;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .welcome {
    max-width: 760px;
    margin: 40px auto 0;
    text-align: center;
  }
  .welcome-title {
    font-family: var(--font-display);
    font-size: 28px;
    font-weight: 600;
    letter-spacing: -0.01em;
    margin: 0 0 6px;
  }
  .welcome-sub {
    color: var(--ink-muted);
    margin: 0 0 24px;
    font-size: 14px;
  }
  .welcome-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 12px;
  }
  .welcome-tile {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 18px;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    cursor: pointer;
    transition: border-color 150ms, box-shadow 150ms;
    text-align: left;
    box-shadow: var(--shadow-sm);
    color: var(--ink);
    font-family: var(--font-ui);
  }
  .welcome-tile:hover {
    border-color: var(--accent);
    box-shadow: var(--shadow-md);
  }
  .welcome-tile h4 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 14px;
    font-weight: 600;
  }
  .welcome-tile p {
    margin: 0;
    font-size: 12px;
    color: var(--ink-muted);
    line-height: 1.5;
  }
  .welcome-tile :global(svg) {
    color: var(--accent);
  }

  .status-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 12px;
    margin-bottom: 24px;
  }
  .status-tile {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 14px 16px;
    box-shadow: var(--shadow-sm);
  }
  .status-head {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ink-muted);
    font-family: var(--font-ui);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-weight: 600;
    margin-bottom: 8px;
  }
  .status-value {
    font-size: 22px;
    font-weight: 600;
    color: var(--ink);
    line-height: 1.1;
    font-variant-numeric: tabular-nums;
  }
  .status-sub {
    margin-top: 4px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-faint);
  }

  .two-col {
    display: grid;
    grid-template-columns: 1.4fr 1fr;
    gap: 16px;
    align-items: start;
  }
  .right-col {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .list-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 5px 0;
    border-bottom: 1px solid var(--line);
    font-size: 13px;
    gap: 12px;
  }
  .list-row:last-child { border-bottom: none; }
  .list-name { color: var(--ink-secondary); }
  .list-count { font-family: var(--font-mono); font-size: 12px; color: var(--ink-faint); }
</style>
