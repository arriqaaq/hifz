<script lang="ts">
  import { onMount } from 'svelte';
  import { getHealth, getDigest, getSessions, getCommits } from '$lib/api';
  import type { HealthResponse, ProjectDigest, Session, Commit } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let health = $state<HealthResponse | null>(null);
  let digest = $state<ProjectDigest | null>(null);
  let sessions = $state<Session[]>([]);
  let commits = $state<Commit[]>([]);
  let loading = $state(true);
  let error = $state('');

  onMount(async () => {
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
  });

  function formatUptime(seconds: number): string {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    if (h > 24) return `${Math.floor(h / 24)}d`;
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m`;
  }

  function formatTime(ts: string): string {
    return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  function projectName(path: string): string {
    return path.split('/').pop() ?? path;
  }
</script>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="card" style="border-color: var(--accent);">
    <div class="card-title" style="color: var(--accent);">Connection Error</div>
    <p>{error}</p>
    <p style="font-size: 12px; color: var(--ink-faint); font-family: var(--font-mono);">Make sure the hifz server is running on port 3111</p>
  </div>
{:else}
  {#if health}
    <div class="stats-grid">
      <div class="stat-card" title="Claude Code conversations from start to end">
        <div class="label">Sessions</div>
        <div class="value">{health.sessions}</div>
        <div class="sub">{sessions.filter(s => !s.ended_at).length} active</div>
      </div>
      <div class="stat-card" title="Individual tool calls, edits, and commands captured from hooks">
        <div class="label">Observations</div>
        <div class="value">{health.observations}</div>
      </div>
      <div class="stat-card" title="Saved insights, patterns, and decisions for long-term recall">
        <div class="label">Memories</div>
        <div class="value">{health.memories}</div>
      </div>
      <div class="stat-card">
        <div class="label">Health</div>
        <div class="value value--status">
          <span class="health-dot healthy"></span>
          {health.status}
        </div>
        <div class="sub">connected</div>
      </div>
      <div class="stat-card">
        <div class="label">Uptime</div>
        <div class="value">{formatUptime(health.uptime_seconds)}</div>
      </div>
      <div class="stat-card">
        <div class="label">Version</div>
        <div class="value value--sm">{health.version}</div>
      </div>
    </div>
  {/if}

  <div class="two-col">
    {#if sessions.length > 0}
      <div class="card">
        <div class="card-title">Recent Sessions</div>
        <table>
          <thead>
            <tr>
              <th>Project</th>
              <th>Status</th>
              <th>Obs</th>
              <th>Started</th>
            </tr>
          </thead>
          <tbody>
            {#each sessions as s}
              <tr>
                <td><a href="/sessions/{s.id}" class="row-link">{projectName(s.project)}</a></td>
                <td>
                  {#if s.ended_at}
                    <span class="badge badge-blue">completed</span>
                  {:else}
                    <span class="badge badge-green">active</span>
                  {/if}
                </td>
                <td class="mono">{s.observation_count}</td>
                <td class="mono faint">{formatTime(s.started_at)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    {#if commits.length > 0}
      <div class="card">
        <div class="card-title">Recent Commits</div>
        <table>
          <thead>
            <tr>
              <th>SHA</th>
              <th>Message</th>
              <th>Branch</th>
              <th>Files</th>
            </tr>
          </thead>
          <tbody>
            {#each commits as c}
              <tr>
                <td class="mono"><a href="/commits/{c.sha}" class="row-link">{c.sha.slice(0, 8)}</a></td>
                <td class="commit-msg">{c.message}</td>
                <td><span class="badge badge-blue">{c.branch}</span></td>
                <td class="mono">{c.files_changed.length}</td>
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

      {#if digest && digest.top_files && digest.top_files.length > 0}
        <div class="card">
          <div class="card-title">Top Files</div>
          {#each digest.top_files.slice(0, 8) as f}
            <div class="list-row">
              <span class="list-name mono">{f.file.split('/').pop()}</span>
              <span class="list-count">{f.frequency}</span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    border: 1px solid var(--border);
    margin-bottom: 24px;
  }

  .two-col {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
    align-items: start;
  }

  .right-col {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .row-link {
    font-weight: 700;
    color: var(--ink);
  }
  .row-link:hover {
    color: var(--accent);
  }

  .mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .faint {
    color: var(--ink-faint);
  }

  .value--status {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 24px;
  }

  .value--sm {
    font-size: 20px;
  }

  .list-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 0;
    border-bottom: 1px solid var(--border-light);
    font-size: 13px;
  }
  .list-row:last-child {
    border-bottom: none;
  }

  .list-name {
    color: var(--ink-secondary);
  }

  .commit-msg {
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }

  .list-count {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--ink-faint);
  }
</style>
