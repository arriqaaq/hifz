<script lang="ts">
  import { onMount } from 'svelte';
  import { listProjects, createProject, type AtlasProject } from '$lib/api';

  // `project` is the selected slug (bindable). `onchange` fires whenever the
  // selection changes (incl. on initial restore) so the parent can refetch.
  let {
    project = $bindable(''),
    onchange,
  }: { project?: string; onchange?: (slug: string) => void } = $props();

  let projects = $state<AtlasProject[]>([]);
  let loading = $state(true);
  let showNew = $state(false);
  let newName = $state('');
  let err = $state('');

  function restore(): string {
    try {
      const u = new URL(window.location.href);
      return u.searchParams.get('project') || localStorage.getItem('atlas.project') || '';
    } catch {
      return '';
    }
  }
  function persist(slug: string) {
    try {
      const u = new URL(window.location.href);
      if (slug) u.searchParams.set('project', slug);
      else u.searchParams.delete('project');
      window.history.replaceState({}, '', u.toString());
      localStorage.setItem('atlas.project', slug);
    } catch {
      /* non-browser context */
    }
  }
  function select(slug: string) {
    project = slug;
    persist(slug);
    onchange?.(slug);
  }

  async function load() {
    loading = true;
    err = '';
    try {
      const r = await listProjects();
      projects = r.projects ?? [];
      const want = project || restore();
      if (want && projects.some((p) => p.slug === want)) select(want);
      else if (projects.length) select(projects[0].slug);
      else select('');
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }
  onMount(load);

  async function create() {
    const name = newName.trim();
    if (!name) return;
    err = '';
    try {
      const p = await createProject(name);
      newName = '';
      showNew = false;
      await load();
      select(p.slug);
    } catch (e) {
      err = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<div class="picker">
  <span class="lbl">Project</span>
  {#if projects.length}
    <select
      value={project}
      onchange={(e) => select((e.currentTarget as HTMLSelectElement).value)}
    >
      {#each projects as p}
        <option value={p.slug}>{p.name}</option>
      {/each}
    </select>
  {:else if !loading}
    <span class="empty-hint">no projects yet — create one</span>
  {/if}

  {#if showNew}
    <input
      class="newname"
      placeholder="project name"
      bind:value={newName}
      onkeydown={(e) => e.key === 'Enter' && create()}
    />
    <button class="btn btn--accent btn--small" onclick={create}>Create</button>
    <button
      class="btn btn--small"
      onclick={() => {
        showNew = false;
        newName = '';
        err = '';
      }}>Cancel</button
    >
  {:else}
    <button class="btn btn--small" onclick={() => (showNew = true)}>+ New project</button>
  {/if}

  {#if err}<span class="err">{err}</span>{/if}
</div>

<style>
  .picker {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .lbl {
    font-size: 13px;
    color: var(--ink-muted);
  }
  .empty-hint {
    font-size: 13px;
    color: var(--ink-faint);
  }
  .newname {
    min-width: 160px;
  }
  .err {
    color: var(--red);
    font-size: 12px;
  }
</style>
