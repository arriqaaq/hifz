<script lang="ts">
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import { getTimeline } from '$lib/api';
  import type { Observation } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let observations = $state<Observation[]>([]);
  let loading = $state(true);
  let error = $state('');

  let sessionId = $derived(decodeURIComponent(page.params.id ?? ''));
  let timelineId = $derived(sessionId.replace(/^session:/, ''));

  onMount(async () => {
    try {
      const res = await getTimeline(timelineId, 200);
      observations = res.observations;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load';
    } finally {
      loading = false;
    }
  });

  function formatTime(ts: string): string {
    return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }
</script>

<a href="/sessions" class="back-link">&larr; All Sessions</a>

<div class="session-header">
  <span class="session-label">Session</span>
  <span class="session-id">{sessionId}</span>
</div>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="card" style="border-color: var(--accent);">
    <p style="color: var(--accent); margin: 0;">{error}</p>
  </div>
{:else if observations.length === 0}
  <p class="empty">No observations recorded</p>
{:else}
  <div class="timeline">
    {#each observations as obs (obs.id)}
      {#if obs.title === 'unknown call' || obs.narrative === 'User submitted a prompt.'}
        <div class="obs-muted">
          <span class="obs-time">{formatTime(obs.timestamp)}</span>
          <span class="badge badge-blue" style="opacity: 0.5">{obs.obs_type}</span>
          <span class="obs-muted-text">{obs.narrative || obs.title}</span>
        </div>
      {:else}
        <div class="card obs-card">
          <div class="obs-header">
            <span class="obs-time">{formatTime(obs.timestamp)}</span>
            <span class="badge badge-blue">{obs.obs_type}</span>
            {#if obs.importance >= 7}
              <span class="obs-imp">&#9733; {obs.importance}</span>
            {/if}
          </div>
          <h4 class="obs-title">{obs.title}</h4>
          {#if obs.subtitle}
            <p class="obs-sub">{obs.subtitle}</p>
          {/if}
          {#if obs.narrative}
            {#if obs.obs_type === 'command_run' || obs.obs_type === 'file_edit' || obs.obs_type === 'file_write'}
              <pre class="obs-code"><code>{obs.narrative}</code></pre>
            {:else}
              <p class="obs-narrative">{obs.narrative}</p>
            {/if}
          {/if}
          {#if obs.facts.length > 0}
            <details class="obs-facts-details">
              <summary class="obs-facts-summary">{obs.facts.length} detail{obs.facts.length > 1 ? 's' : ''}</summary>
              <ul class="obs-facts">
                {#each obs.facts as fact}
                  <li><code class="fact-code">{fact}</code></li>
                {/each}
              </ul>
            </details>
          {/if}
          {#if obs.concepts.length > 0 || obs.files.length > 0}
            <div class="obs-tags">
              {#each obs.concepts as c}
                <span class="badge badge-yellow">{c}</span>
              {/each}
              {#each obs.files as f}
                <span class="badge badge-cyan" style="font-family: var(--font-mono); font-size: 9px">{f.split('/').pop()}</span>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .back-link {
    display: inline-block;
    font-size: 11px;
    font-weight: 600;
    font-family: var(--font-ui);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-muted);
    margin-bottom: 16px;
    transition: color 150ms;
  }
  .back-link:hover { color: var(--accent); }

  .session-header {
    margin-bottom: 24px;
  }
  .session-label {
    display: block;
    font-size: 9px;
    font-weight: 600;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--ink-muted);
    margin-bottom: 4px;
  }
  .session-id {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--ink-faint);
  }

  .timeline {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 860px;
  }

  .obs-card {
    padding: 16px 20px;
  }

  .obs-header {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
  }

  .obs-time {
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 500;
    color: var(--ink-muted);
  }

  .obs-imp {
    font-size: 11px;
    color: var(--yellow);
    margin-left: auto;
  }

  .obs-title {
    margin: 0 0 4px;
    font-size: 14px;
    font-weight: 700;
    font-family: var(--font-display);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .obs-sub {
    margin: 0 0 6px;
    font-size: 12px;
    color: var(--ink-muted);
    font-family: var(--font-ui);
  }

  .obs-narrative {
    margin: 0 0 10px;
    font-size: 13px;
    color: var(--ink-secondary);
    line-height: 1.6;
  }

  .obs-facts {
    margin: 0 0 10px;
    padding-left: 18px;
    font-size: 12px;
    color: var(--ink-secondary);
    line-height: 1.6;
  }
  .obs-facts li { margin-bottom: 2px; }

  .obs-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .obs-code {
    margin: 0 0 10px;
    padding: 10px 14px;
    background: var(--bg-alt, #F0F0EC);
    border: 1px solid var(--border-light);
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.5;
    color: var(--ink-secondary);
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .obs-code code {
    font-family: inherit;
    font-size: inherit;
  }

  .obs-facts-details {
    margin: 0 0 10px;
  }
  .obs-facts-summary {
    font-family: var(--font-ui);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-faint);
    cursor: pointer;
    padding: 4px 0;
  }
  .obs-facts-summary:hover { color: var(--ink-muted); }

  .fact-code {
    font-family: var(--font-mono);
    font-size: 10px;
    background: var(--bg-alt, #F0F0EC);
    padding: 1px 4px;
    border-radius: 2px;
  }

  .obs-muted {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    font-size: 11px;
    color: var(--ink-faint);
    border-left: 2px solid var(--border-light);
  }
  .obs-muted-text {
    font-family: var(--font-ui);
    font-size: 11px;
    color: var(--ink-faint);
  }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }
</style>
