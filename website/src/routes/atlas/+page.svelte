<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getSessions,
    getAtlasInsights,
    atlasQuery,
    atlasStatus,
    atlasBuildAll,
    atlasCode,
    atlasIngest,
    atlasExtract,
    atlasCluster,
    atlasUpload,
    type AtlasInsights,
    type AtlasHit,
    type AtlasStatus,
  } from '$lib/api';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let project = $state('default');
  let projects = $state<string[]>([]);

  let ins = $state<AtlasInsights | null>(null);
  let loading = $state(true);
  let error = $state('');

  let q = $state('');
  let hits = $state<AtlasHit[]>([]);
  let searching = $state(false);

  // Build panel
  let source = $state<'path' | 'git' | 'upload'>('path');
  let codePath = $state('');
  let gitUrl = $state('');
  let docsPath = $state('');
  let fileList = $state<FileList | null>(null);
  let buildLog = $state<string[]>([]);
  let job = $state<AtlasStatus | null>(null);
  let polling = $state(false);

  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  async function refresh() {
    loading = true;
    error = '';
    try {
      ins = await getAtlasInsights(project);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(async () => {
    try {
      const res = await getSessions(200);
      const set = new Set<string>();
      for (const s of res.sessions ?? []) if (s?.project) set.add(s.project);
      projects = Array.from(set).sort();
      if (!projects.includes(project) && projects.length) project = projects[0];
    } catch {
      /* projects list is best-effort; fall back to "default" */
    }
    await refresh();
    await pollOnce();
  });

  async function pollOnce() {
    try {
      job = await atlasStatus(project);
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
        job = await atlasStatus(project);
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

  const buildAll = () =>
    kick(
      () =>
        atlasBuildAll(project, {
          path: source === 'path' ? codePath || undefined : undefined,
          git: source === 'git' ? gitUrl || undefined : undefined,
          docs: docsPath || undefined,
        }),
      'build all',
    );
  const indexCode = () =>
    kick(
      () =>
        atlasCode(project, {
          path: source === 'path' ? codePath || undefined : undefined,
          git: source === 'git' ? gitUrl || undefined : undefined,
        }),
      'index code',
    );
  const ingestDocs = () => kick(() => atlasIngest(project, docsPath), 'ingest docs');
  const doExtract = () => kick(() => atlasExtract(project), 'extract concepts');
  const doCluster = () => kick(() => atlasCluster(project), 'cluster');
  function doUpload() {
    if (!fileList || !fileList.length) return;
    kick(() => atlasUpload(project, fileList as FileList), 'upload + ingest');
  }

  async function runQuery() {
    if (!q.trim()) return;
    searching = true;
    try {
      const r = await atlasQuery(project, q.trim());
      hits = r.hits ?? [];
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      searching = false;
    }
  }
</script>

<div class="page">
  <header class="head">
    <h1>Atlas</h1>
    <p class="sub">Corpus knowledge graph — documents, concepts, and the code graph, clustered.</p>
  </header>

  <div class="card toolbar">
    <label>
      Project
      <select bind:value={project} onchange={() => { refresh(); pollOnce(); }}>
        {#if !projects.includes(project)}<option value={project}>{project}</option>{/if}
        {#each projects as p}<option value={p}>{p}</option>{/each}
      </select>
    </label>
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

  {#if loading}
    <LoadingSpinner />
  {:else if error}
    <div class="card"><span class="badge badge-red">error</span> {error}</div>
  {:else if ins && ins.nodes === 0}
    <div class="card empty">
      <div class="card-title">Atlas is empty for “{project}”</div>
      <p class="sub">
        Point Atlas at a repository or docs folder above (server path or git URL),
        or upload PDFs/markdown, then <strong>Build all</strong>. Status shows in the
        toolbar; the graph populates when the job finishes.
      </p>
    </div>
  {:else if ins}
    <div class="stats">
      <div class="stat-card"><div class="label">Nodes</div><div class="value">{ins.nodes}</div></div>
      <div class="stat-card"><div class="label">Edges</div><div class="value">{ins.edges}</div></div>
      <div class="stat-card"><div class="label">Clusters</div><div class="value">{ins.clusters}</div></div>
    </div>

    <div class="card">
      <div class="card-title">Query</div>
      <div class="qrow">
        <input
          placeholder="search the corpus graph…"
          bind:value={q}
          onkeydown={(e) => e.key === 'Enter' && runQuery()}
        />
        <button class="btn btn--accent" onclick={runQuery} disabled={searching}>
          {searching ? '…' : 'Search'}
        </button>
      </div>
      {#if hits.length}
        <table>
          <thead><tr><th>Kind</th><th>Label</th><th>Snippet</th></tr></thead>
          <tbody>
            {#each hits as h}
              <tr>
                <td><span class="badge badge-blue">{h.kind}</span></td>
                <td class="mono">{h.label}</td>
                <td>{h.snippet ?? ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>

    <div class="card">
      <div class="card-title">Hub nodes — most connected</div>
      <table>
        <thead><tr><th>Label</th><th>Kind</th><th>Weighted degree</th><th>Clusters touched</th></tr></thead>
        <tbody>
          {#each ins.hub_nodes as h}
            <tr>
              <td class="mono">{h.label}</td>
              <td><span class="badge badge-purple">{h.kind}</span></td>
              <td>{h.weighted_degree.toFixed(2)}</td>
              <td>{h.clusters_touched}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="card">
      <div class="card-title">Surprising cross-cluster links</div>
      <table>
        <thead><tr><th>From</th><th>Relation</th><th>To</th><th>Why</th></tr></thead>
        <tbody>
          {#each ins.surprising_links as s}
            <tr>
              <td class="mono">{s.from}</td>
              <td><span class="badge badge-orange">{s.relation}</span></td>
              <td class="mono">{s.to}</td>
              <td>{s.why}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="card">
      <div class="card-title">Isolated nodes — gaps</div>
      <table>
        <thead><tr><th>Label</th><th>Kind</th><th>Degree</th></tr></thead>
        <tbody>
          {#each ins.isolated_nodes as i}
            <tr>
              <td class="mono">{i.label}</td>
              <td><span class="badge badge-gray">{i.kind}</span></td>
              <td>{i.weighted_degree.toFixed(2)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .page { padding: 24px; max-width: 1100px; margin: 0 auto; }
  .head { margin-bottom: 18px; }
  .head h1 { margin: 0; font-size: 22px; font-weight: 700; }
  .head .sub { color: var(--ink-muted); font-size: 13px; margin: 4px 0 0; }
  .toolbar { display: flex; align-items: center; gap: 16px; }
  .toolbar label { display: flex; align-items: center; gap: 8px; font-size: 13px; }
  .toolbar .status { margin-left: auto; }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 14px; }
  .qrow { display: flex; gap: 8px; margin-bottom: 12px; }
  .qrow input { flex: 1; }
  .srcrow { display: flex; gap: 8px; flex-wrap: wrap; margin: 8px 0; }
  .wide { width: 100%; margin: 6px 0; }
  .log { background: var(--bg-elev, #f2f1eb); padding: 10px; font-size: 12px; overflow-x: auto; white-space: pre-wrap; }
  .empty .sub { margin: 6px 0 0; }
</style>
