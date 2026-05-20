<script lang="ts">
  import {
    getMaktabInsights,
    maktabStatus,
    maktabBuildAll,
    maktabCode,
    maktabIngest,
    maktabExtract,
    maktabCluster,
    maktabUpload,
    codeIndex,
    type MaktabInsights,
    type MaktabStatus,
  } from '$lib/api';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';
  import ProjectPicker from '$lib/components/maktab/ProjectPicker.svelte';

  // Selected project slug — owned by the ProjectPicker (create-first; no
  // implicit 'default'). Ask/Search now live on the dedicated /ask page.
  let project = $state('');

  let ins = $state<MaktabInsights | null>(null);
  let loading = $state(true);
  let error = $state('');

  // Build panel
  let source = $state<'path' | 'git' | 'upload'>('path');
  let codePath = $state('');
  let gitUrl = $state('');
  let docsPath = $state('');
  let fileList = $state<FileList | null>(null);
  let buildLog = $state<string[]>([]);
  let job = $state<MaktabStatus | null>(null);
  let polling = $state(false);

  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  async function refresh() {
    if (!project) {
      ins = null;
      loading = false;
      return;
    }
    loading = true;
    error = '';
    try {
      // Time-bound the insights load so a large / un-rebuilt graph can never
      // hang the panel forever. 12s is generous for a healthy graph.
      const timeout = new Promise<never>((_, reject) =>
        setTimeout(
          () =>
            reject(
              new Error(
                'insights still loading (a build/index job may be running, or the graph is large).',
              ),
            ),
          12000,
        ),
      );
      ins = await Promise.race([getMaktabInsights(project), timeout]);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  // Driven by the ProjectPicker (fires on initial restore + every change).
  async function onProjectChange() {
    await refresh();
    await pollOnce();
  }

  async function pollOnce() {
    if (!project) {
      job = null;
      return;
    }
    try {
      job = await maktabStatus(project);
    } catch {
      /* ignore */
    }
  }

  // Poll until the background build finishes, then log + refresh insights.
  async function pollUntilDone() {
    if (polling) return;
    polling = true;
    try {
      // give the spawned job a beat to flip `running`
      await sleep(800);
      for (;;) {
        job = await maktabStatus(project);
        if (!job.running) break;
        await sleep(2000);
      }
      if (job?.error) buildLog = [...buildLog, `error: ${job.error}`];
      else if (job?.last_report)
        buildLog = [...buildLog, JSON.stringify(job.last_report)];
      await refresh();
    } catch (e) {
      buildLog = [...buildLog, `status poll failed: ${e instanceof Error ? e.message : e}`];
    } finally {
      polling = false;
    }
  }

  async function kick(
    fn: () => Promise<{ started?: boolean; error?: string }>,
    label: string,
  ) {
    if (!project) {
      buildLog = [...buildLog, `✗ ${label}: create or select a project first`];
      return;
    }
    buildLog = [...buildLog, `▶ ${label}…`];
    try {
      const r = await fn();
      if (r.error) {
        buildLog = [...buildLog, `✗ ${r.error}`];
        return;
      }
      await pollUntilDone();
    } catch (e) {
      buildLog = [...buildLog, `✗ ${e instanceof Error ? e.message : String(e)}`];
    }
  }

  // Code is parsed ONCE by hifz core (`/code/index`); Maktab then projects the
  // corpus graph from core. Run the core index first so the Maktab job reads it
  // instead of re-walking the repo. Returns false on failure (abort the build).
  async function coreIndex(): Promise<boolean> {
    if (source === 'upload') return true; // uploads are docs, not code
    const path = source === 'path' ? codePath.trim() : '';
    const git = source === 'git' ? gitUrl.trim() : '';
    if (!path && !git) return true; // docs-only build, nothing to index
    buildLog = [...buildLog, '▶ index code (core)…'];
    try {
      const r = await codeIndex(project, path, { git: git || undefined });
      buildLog = [
        ...buildLog,
        `  core: ${r.indexed} files · ${r.chunks} chunks · ${r.symbols} symbols`,
      ];
      return true;
    } catch (e) {
      buildLog = [...buildLog, `✗ core index: ${e instanceof Error ? e.message : String(e)}`];
      return false;
    }
  }

  const buildAll = async () => {
    if (!project) {
      buildLog = [...buildLog, '✗ build all: create or select a project first'];
      return;
    }
    if (!(await coreIndex())) return;
    await kick(
      () =>
        maktabBuildAll(project, {
          path: source === 'path' ? codePath || undefined : undefined,
          git: source === 'git' ? gitUrl || undefined : undefined,
          docs: docsPath || undefined,
        }),
      'build all (projects code from core)',
    );
  };
  const indexCode = async () => {
    if (!project) {
      buildLog = [...buildLog, '✗ index code: create or select a project first'];
      return;
    }
    if (!(await coreIndex())) return;
    await kick(
      () =>
        maktabCode(project, {
          path: source === 'path' ? codePath || undefined : undefined,
          git: source === 'git' ? gitUrl || undefined : undefined,
        }),
      'project code → corpus graph',
    );
  };
  const ingestDocs = () => kick(() => maktabIngest(project, docsPath), 'ingest docs');
  const doExtract = () => kick(() => maktabExtract(project), 'extract concepts');
  const doCluster = () => kick(() => maktabCluster(project), 'cluster');
  function doUpload() {
    if (!fileList || !fileList.length) return;
    kick(() => maktabUpload(project, fileList as FileList), 'upload + ingest');
  }
</script>

<div class="page">
  <header class="head">
    <h1>Maktab</h1>
    <p class="sub">Corpus knowledge graph — documents, concepts, and the code graph, clustered.</p>
  </header>

  <div class="card toolbar">
    <ProjectPicker bind:project onchange={onProjectChange} />
    <span class="status">
      {#if job?.running}
        <span class="badge badge-orange">building: {job.step}</span>
      {:else if job?.error}
        <span class="badge badge-red">error</span>
      {:else}
        <span class="badge badge-gray">idle</span>
      {/if}
    </span>
  </div>

  <div class="card">
    <div class="card-title">Build</div>
    <div class="srcrow">
      <button class="btn" class:btn--accent={source === 'path'} onclick={() => (source = 'path')}>Codebase path</button>
      <button class="btn" class:btn--accent={source === 'git'} onclick={() => (source = 'git')}>Git URL</button>
      <button class="btn" class:btn--accent={source === 'upload'} onclick={() => (source = 'upload')}>Upload</button>
    </div>
    {#if source === 'path'}
      <input class="wide" placeholder="/abs/path/to/repo (server-side, daemon-readable)" bind:value={codePath} />
    {:else if source === 'git'}
      <input class="wide" placeholder="https://github.com/owner/repo.git" bind:value={gitUrl} />
    {:else}
      <input class="wide" type="file" multiple onchange={(e) => (fileList = (e.currentTarget as HTMLInputElement).files)} />
    {/if}
    <input class="wide" placeholder="optional docs/PDF folder (server-side)" bind:value={docsPath} />
    <div class="srcrow">
      {#if source === 'upload'}
        <button class="btn btn--accent" onclick={doUpload} disabled={polling}>Upload + ingest</button>
      {:else}
        <button class="btn btn--accent" onclick={buildAll} disabled={polling}>Build all</button>
        <button class="btn" onclick={indexCode} disabled={polling}>Index code</button>
      {/if}
      <button class="btn" onclick={ingestDocs} disabled={polling || !docsPath}>Ingest docs</button>
      <button class="btn" onclick={doExtract} disabled={polling}>Extract</button>
      <button class="btn" onclick={doCluster} disabled={polling}>Cluster</button>
    </div>
    {#if buildLog.length}
      <pre class="log">{buildLog.slice(-12).join('\n')}</pre>
    {/if}
  </div>

  {#if !project}
    <div class="card empty">
      <div class="card-title">No project selected</div>
      <p class="sub">
        Maktab is project-first — create a project above, then point it at a
        repo / docs folder and <strong>Build all</strong>. Ask its corpus on the
        <a href="/ask">Ask</a> page.
      </p>
    </div>
  {:else if loading}
    <LoadingSpinner />
  {:else if error}
    <div class="card"><span class="badge badge-red">error</span> {error}</div>
  {:else if ins && ins.nodes === 0}
    <div class="card empty">
      <div class="card-title">Maktab is empty for “{project}”</div>
      <p class="sub">
        No corpus for this project yet — point Maktab at a repo/docs folder
        above and <strong>Build all</strong>. Then <a href={`/ask?project=${encodeURIComponent(project)}`}>Ask</a> its corpus.
      </p>
    </div>
  {:else if ins}
    <div class="stats">
      <div class="stat-card"><div class="label">Nodes</div><div class="value">{ins.nodes}</div></div>
      <div class="stat-card"><div class="label">Edges</div><div class="value">{ins.edges}</div></div>
      <div class="stat-card"><div class="label">Clusters</div><div class="value">{ins.clusters}</div></div>
    </div>
  {/if}
</div>

<style>
  .page { padding: 24px; max-width: 1100px; margin: 0 auto; }
  .head { margin-bottom: 18px; }
  .head h1 { margin: 0; font-size: 22px; font-weight: 700; }
  .head .sub { color: var(--ink-muted); font-size: 13px; margin: 4px 0 0; }
  .toolbar { display: flex; align-items: center; gap: 16px; }
  .toolbar .status { margin-left: auto; }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 14px; }
  .srcrow { display: flex; gap: 8px; flex-wrap: wrap; margin: 8px 0; }
  .wide { width: 100%; margin: 6px 0; }
  .log { background: var(--bg-elev, #f2f1eb); padding: 10px; font-size: 12px; overflow-x: auto; white-space: pre-wrap; }
  .empty .sub { margin: 6px 0 0; }
</style>
