<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { getProjectAccumulators, getProjectDigest } from '$lib/api';
  import type { WarmupDigest, ProjectDigestByCategory } from '$lib/types';
  import { categoryColor, categoryLabel } from '$lib/ontology';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let projectName = $derived($page.params.project ?? '');

  let acc = $state<WarmupDigest | null>(null);
  let digest = $state<ProjectDigestByCategory | null>(null);
  let days = $state(30);
  let loading = $state(true);
  let error = $state('');

  async function load() {
    loading = true;
    error = '';
    try {
      const [a, d] = await Promise.all([
        getProjectAccumulators(projectName),
        getProjectDigest(projectName, days),
      ]);
      acc = a;
      digest = d;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Load failed';
    } finally {
      loading = false;
    }
  }

  onMount(load);

  function changeDays(n: number) {
    days = n;
    load();
  }

  function formatDate(ts: string): string {
    return new Date(ts).toLocaleDateString([], { month: 'short', day: 'numeric' });
  }
</script>

<div class="page">
  <header class="page-head">
    <div>
      <p class="path-crumb">project</p>
      <h1>{projectName}</h1>
    </div>
    <nav class="day-toggle">
      {#each [7, 30, 90] as n}
        <button class:active={days === n} onclick={() => changeDays(n)}>{n}d</button>
      {/each}
    </nav>
  </header>

  {#if error}
    <div class="card error">{error}</div>
  {/if}

  {#if loading}
    <LoadingSpinner />
  {:else}
    <!-- Accumulators row -->
    <section class="grid">
      <!-- Active plan -->
      <div class="card big">
        <h2>Active plan</h2>
        {#if acc?.latest_plan}
          <a href={`/memories/${encodeURIComponent(acc.latest_plan.id)}`} class="plan-link">
            <h3>{acc.latest_plan.title}</h3>
            <p>{acc.latest_plan.summary}</p>
          </a>
        {:else}
          <p class="empty">No active plan in this project.</p>
        {/if}
      </div>

      <!-- Open bugs -->
      <div class="card">
        <h2>
          Open bugs <span class="count">{acc?.open_bugs?.length ?? 0}</span>
        </h2>
        {#if acc?.open_bugs?.length}
          <ul>
            {#each acc.open_bugs as e}
              <li>
                <a href={`/memories/${encodeURIComponent(e.id)}`}>{e.title}</a>
                <p class="entry-summary">{e.summary}</p>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="empty">No open bugs.</p>
        {/if}
      </div>

      <!-- Recent decisions -->
      <div class="card">
        <h2>
          Decisions <span class="count">{acc?.decisions?.length ?? 0}</span>
        </h2>
        {#if acc?.decisions?.length}
          <ul>
            {#each acc.decisions as e}
              <li>
                <a href={`/memories/${encodeURIComponent(e.id)}`}>{e.title}</a>
                <p class="entry-summary">{e.summary}</p>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="empty">No decisions recorded.</p>
        {/if}
      </div>

      <!-- Conventions -->
      <div class="card">
        <h2>
          Conventions <span class="count">{acc?.conventions?.length ?? 0}</span>
        </h2>
        {#if acc?.conventions?.length}
          <ul>
            {#each acc.conventions as e}
              <li>
                <a href={`/memories/${encodeURIComponent(e.id)}`}>{e.title}</a>
                <p class="entry-summary">{e.summary}</p>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="empty">No conventions recorded.</p>
        {/if}
      </div>

      <!-- Gotchas -->
      <div class="card">
        <h2>
          Gotchas <span class="count">{acc?.gotchas?.length ?? 0}</span>
        </h2>
        {#if acc?.gotchas?.length}
          <ul>
            {#each acc.gotchas as e}
              <li>
                <a href={`/memories/${encodeURIComponent(e.id)}`}>{e.title}</a>
                <p class="entry-summary">{e.summary}</p>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="empty">No gotchas recorded.</p>
        {/if}
      </div>

      <!-- Failure patterns -->
      <div class="card">
        <h2>
          Failure patterns <span class="count">{acc?.failure_patterns?.length ?? 0}</span>
        </h2>
        {#if acc?.failure_patterns?.length}
          <ul>
            {#each acc.failure_patterns as e}
              <li>
                <a href={`/memories/${encodeURIComponent(e.id)}`}>{e.title}</a>
                <p class="entry-summary">{e.summary}</p>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="empty">No failure patterns recorded.</p>
        {/if}
      </div>

      <!-- Recent lessons -->
      <div class="card">
        <h2>
          Recent lessons <span class="count">{acc?.recent_lessons?.length ?? 0}</span>
        </h2>
        {#if acc?.recent_lessons?.length}
          <ul>
            {#each acc.recent_lessons as e}
              <li>
                <a href={`/memories/${encodeURIComponent(e.id)}`}>{e.title}</a>
                <p class="entry-summary">{e.summary}</p>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="empty">No lessons yet.</p>
        {/if}
      </div>
    </section>

    <!-- Chronological digest (last N days) -->
    <section class="digest">
      <h2 class="section-h">Activity — last {days} days</h2>
      {#if digest && Object.keys(digest.by_category).length > 0}
        <div class="cat-grid">
          {#each Object.entries(digest.by_category) as [cat, entries]}
            <div class="card">
              <h3>
                <span class="badge {categoryColor(cat)}">{categoryLabel(cat)}</span>
                <span class="count">{entries.length}</span>
              </h3>
              <ul>
                {#each entries as e}
                  <li>
                    <a href={`/memories/${encodeURIComponent(e.id)}`}>{e.title}</a>
                    <span class="when">{formatDate(e.created_at)}</span>
                  </li>
                {/each}
              </ul>
            </div>
          {/each}
        </div>
      {:else}
        <p class="empty">No activity in the last {days} days.</p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .page { padding: 0; }
  .page-head { display: flex; align-items: end; gap: 16px; margin-bottom: 16px; }
  .page-head h1 { margin: 0; font-size: 24px; }
  .path-crumb { margin: 0; font-size: 11px; text-transform: uppercase; color: var(--ink-faint); letter-spacing: 0.05em; }
  .day-toggle { margin-left: auto; display: flex; border: 1.5px solid var(--ink); border-radius: var(--radius-sm); overflow: hidden; }
  .day-toggle button { padding: 6px 12px; border: none; background: var(--surface); cursor: pointer; font-size: 11px; font-family: var(--font-mono); font-weight: 600; color: var(--ink); }
  .day-toggle button:hover { background: var(--surface-alt); }
  .day-toggle button.active { background: var(--neon); color: var(--ink); }
  .day-toggle button + button { border-left: 1px solid var(--ink); }

  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; margin-bottom: 24px; }
  .card { padding: 12px; }
  .card.big { grid-column: span 2; }
  @media (max-width: 700px) { .card.big { grid-column: span 1; } }

  .card h2 { margin: 0 0 8px; font-size: 12px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--ink-faint); display: flex; align-items: center; gap: 6px; }
  .count { font-family: var(--font-mono); font-size: 10px; color: var(--ink-faint); }

  .plan-link { display: block; text-decoration: none; color: var(--ink); }
  .plan-link h3 { margin: 0 0 4px; font-size: 16px; }
  .plan-link p { margin: 0; color: var(--ink-muted); font-size: 12px; }
  .plan-link:hover h3 { color: var(--ink); text-decoration: underline; text-decoration-color: var(--neon); text-decoration-thickness: 2px; text-underline-offset: 2px; }

  ul { list-style: none; margin: 0; padding: 0; }
  li { padding: 6px 0; border-bottom: 1px solid var(--line); }
  li:last-child { border-bottom: none; }
  li a { color: var(--ink); text-decoration: none; font-weight: 600; font-size: 12px; }
  li a:hover { color: var(--ink); text-decoration: underline; text-decoration-color: var(--neon); text-decoration-thickness: 2px; text-underline-offset: 2px; }
  .entry-summary { margin: 2px 0 0; color: var(--ink-muted); font-size: 11px; line-height: 1.4; }
  .when { float: right; font-family: var(--font-mono); font-size: 10px; color: var(--ink-faint); }

  .empty { color: var(--ink-faint); font-size: 11px; font-style: italic; margin: 0; }
  .error { border-color: var(--danger); color: var(--danger); padding: 12px; }

  .section-h { margin: 24px 0 8px; font-size: 14px; }
  .cat-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; }

  /* badges inherit from global app.css .badge / .badge-* */
</style>
