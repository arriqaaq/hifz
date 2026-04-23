<script lang="ts">
  import { page } from '$app/state';
  import { onMount } from 'svelte';
  import { getRun } from '$lib/api';
  import type { Run, Observation } from '$lib/types';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let run = $state<Run | null>(null);
  let observations = $state<Observation[]>([]);
  let loading = $state(true);
  let error = $state('');

  let runId = $derived(decodeURIComponent(page.params.id ?? ''));

  onMount(async () => {
    try {
      const res = await getRun(runId);
      run = res.run;
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

  function formatDate(ts: string): string {
    return new Date(ts).toLocaleDateString([], { month: 'short', day: 'numeric', year: 'numeric' });
  }

  function extractSessionId(sessionId: unknown): string {
    if (typeof sessionId === 'string') return sessionId.replace(/^session:/, '');
    if (sessionId && typeof sessionId === 'object' && 'key' in sessionId) {
      const key = (sessionId as { key: unknown }).key;
      if (typeof key === 'string') return key;
      if (key && typeof key === 'object' && 'String' in key) {
        return (key as { String: string }).String;
      }
    }
    return String(sessionId);
  }

  function outcomeClass(outcome: string): string {
    switch (outcome) {
      case 'committed': return 'badge-green';
      case 'success': return 'badge-blue';
      case 'uncommitted': return 'badge-yellow';
      case 'failure': return 'badge-red';
      default: return '';
    }
  }
</script>

<a href="/runs" class="back-link">&larr; All Runs</a>

{#if loading}
  <LoadingSpinner />
{:else if error}
  <div class="card" style="border-color: var(--accent);">
    <p style="color: var(--accent); margin: 0;">{error}</p>
  </div>
{:else if run}
  <div class="run-header">
    <div class="run-meta">
      <span class="badge {outcomeClass(run.outcome)}">{run.outcome}</span>
      {#if run.ended_at}
        <span class="meta-item">{formatDate(run.started_at)} {formatTime(run.started_at)} — {formatTime(run.ended_at)}</span>
      {:else}
        <span class="meta-item badge badge-green">in progress</span>
      {/if}
      <a href="/sessions/{extractSessionId(run.session_id)}" class="meta-link">View Session</a>
    </div>
  </div>

  <div class="card section">
    <h3 class="section-title">Prompt</h3>
    <p class="prompt-text">{run.prompt}</p>
    {#if run.prompts && run.prompts.length > 0}
      <details class="follow-up-prompts">
        <summary>{run.prompts.length} follow-up prompt{run.prompts.length > 1 ? 's' : ''}</summary>
        <ul>
          {#each run.prompts as p}
            <li>{p}</li>
          {/each}
        </ul>
      </details>
    {/if}
  </div>

  {#if run.lesson}
    <div class="card section">
      <h3 class="section-title">Lesson</h3>
      <p class="lesson-text">{run.lesson}</p>
    </div>
  {/if}

  <div class="section">
    <h3 class="section-title">Observations ({observations.length})</h3>
    {#if observations.length === 0}
      <p class="empty">No observations recorded for this run.</p>
    {:else}
      <div class="timeline">
        {#each observations as obs (obs.id)}
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
            {#if obs.facts && obs.facts.length > 0}
              <details class="obs-facts-details">
                <summary class="obs-facts-summary">{obs.facts.length} detail{obs.facts.length > 1 ? 's' : ''}</summary>
                <ul class="obs-facts">
                  {#each obs.facts as fact}
                    <li><code class="fact-code">{fact}</code></li>
                  {/each}
                </ul>
              </details>
            {/if}
            {#if (obs.keywords && obs.keywords.length > 0) || (obs.files && obs.files.length > 0)}
              <div class="obs-tags">
                {#each obs.keywords || [] as c}
                  <span class="badge badge-yellow">{c}</span>
                {/each}
                {#each obs.files || [] as f}
                  <span class="badge badge-cyan" style="font-family: var(--font-mono); font-size: 9px">{f.split('/').pop()}</span>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
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

  .run-header {
    margin-bottom: 20px;
  }

  .run-meta {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .meta-item {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--ink-faint);
  }

  .meta-link {
    font-size: 11px;
    font-weight: 600;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-muted);
    margin-left: auto;
  }
  .meta-link:hover { color: var(--accent); }

  .section {
    margin-bottom: 24px;
  }

  .section-title {
    font-family: var(--font-ui);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.12em;
    color: var(--ink-muted);
    margin: 0 0 12px;
  }

  .prompt-text {
    margin: 0;
    font-size: 16px;
    font-weight: 500;
    line-height: 1.5;
  }

  .follow-up-prompts {
    margin-top: 12px;
  }
  .follow-up-prompts summary {
    font-family: var(--font-ui);
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--ink-faint);
    cursor: pointer;
  }
  .follow-up-prompts ul {
    margin: 8px 0 0;
    padding-left: 20px;
    font-size: 13px;
    color: var(--ink-secondary);
  }
  .follow-up-prompts li {
    margin-bottom: 4px;
  }

  .lesson-text {
    margin: 0;
    font-size: 14px;
    line-height: 1.6;
    color: var(--ink-secondary);
    font-style: italic;
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

  .obs-facts {
    margin: 0 0 10px;
    padding-left: 18px;
    font-size: 12px;
    color: var(--ink-secondary);
    line-height: 1.6;
  }
  .obs-facts li { margin-bottom: 2px; }

  .fact-code {
    font-family: var(--font-mono);
    font-size: 10px;
    background: var(--bg-alt, #F0F0EC);
    padding: 1px 4px;
    border-radius: 2px;
  }

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
    max-height: 300px;
  }
  .obs-code code {
    font-family: inherit;
    font-size: inherit;
  }

  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }
</style>
