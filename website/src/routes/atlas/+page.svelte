<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getSessions,
    getAtlasInsights,
    atlasQuery,
    atlasAnswer,
    atlasStatus,
    atlasBuildAll,
    atlasCode,
    atlasIngest,
    atlasExtract,
    atlasCluster,
    atlasUpload,
    type AtlasInsights,
    type AtlasHit,
    type AtlasAnswer,
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
  let ans = $state<AtlasAnswer | null>(null);
  let asking = $state(false);
  let copied = $state<string | null>(null);

  // Split an answer into plain text + [n] citation refs so we can render
  // the refs as anchors without {@html} (LLM output is untrusted).
  type Seg = { t: 'text'; v: string } | { t: 'ref'; n: number };
  function segments(a: string): Seg[] {
    const out: Seg[] = [];
    const re = /\[(\d+)\]/g;
    let last = 0;
    let m: RegExpExecArray | null;
    while ((m = re.exec(a))) {
      if (m.index > last) out.push({ t: 'text', v: a.slice(last, m.index) });
      out.push({ t: 'ref', n: Number(m[1]) });
      last = m.index + m[0].length;
    }
    if (last < a.length) out.push({ t: 'text', v: a.slice(last) });
    return out;
  }
  const isLink = (u?: string | null) => !!u && /^https?:\/\//.test(u);
  async function copyPath(uri: string) {
    try {
      await navigator.clipboard.writeText(uri);
      copied = uri;
      setTimeout(() => (copied = copied === uri ? null : copied), 1500);
    } catch {
      /* clipboard may be blocked; non-fatal */
    }
  }

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
    ans = null;
    try {
      const r = await atlasQuery(project, q.trim());
      hits = r.hits ?? [];
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      searching = false;
    }
  }

  async function runAnswer() {
    if (!q.trim()) return;
    asking = true;
    hits = [];
    try {
      ans = await atlasAnswer(project, q.trim());
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      asking = false;
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
      <div class="card-title">Ask</div>
      <div class="qrow">
        <input
          placeholder="ask a question, or search the corpus…"
          bind:value={q}
          onkeydown={(e) => e.key === 'Enter' && runAnswer()}
        />
        <button class="btn btn--accent" onclick={runAnswer} disabled={asking || searching}>
          {asking ? '…' : 'Ask'}
        </button>
        <button class="btn" onclick={runQuery} disabled={asking || searching}>
          {searching ? '…' : 'Search'}
        </button>
      </div>

      {#if ans}
        {#if ans.note}
          <div class="note"><span class="badge badge-amber">note</span> {ans.note}</div>
        {/if}
        {#if ans.answer}
          <p class="ans">
            {#each segments(ans.answer) as seg}
              {#if seg.t === 'text'}{seg.v}{:else}<a class="cite" href={`#atlas-src-${seg.n}`}
                  >[{seg.n}]</a
                >{/if}
            {/each}
          </p>
        {/if}
        {#if ans.citations.length}
          <div class="card-title">Sources</div>
          <div class="srclist">
            {#each ans.citations as c}
              <div class="src" id={`atlas-src-${c.n}`}>
                <div class="srchead">
                  <span class="cite">[{c.n}]</span>
                  <span class="badge badge-blue">{c.source_kind ?? 'doc'}</span>
                  {#if isLink(c.source_uri)}
                    <a class="ref" href={c.source_uri} target="_blank" rel="noreferrer"
                      >{c.source_ref ?? c.doc_label}</a
                    >
                  {:else}
                    <span class="ref mono">{c.source_ref ?? c.doc_label}</span>
                    {#if c.source_uri}
                      <button class="btn btn--small" onclick={() => copyPath(c.source_uri!)}>
                        {copied === c.source_uri ? 'copied' : 'copy path'}
                      </button>
                    {/if}
                  {/if}
                  {#if c.location}<span class="loc">· {c.location}</span>{/if}
                </div>
                {#if c.snippet}<div class="snip">{c.snippet}</div>{/if}
              </div>
            {/each}
          </div>
        {/if}
      {/if}

      {#if hits.length}
        <table>
          <thead><tr><th>Kind</th><th>Source</th><th>Loc</th><th>Score</th><th>Snippet</th></tr></thead>
          <tbody>
            {#each hits as h}
              <tr>
                <td><span class="badge badge-blue">{h.kind}</span></td>
                <td class="mono">{h.source_ref ?? h.doc_label}</td>
                <td>{h.location ?? ''}</td>
                <td>{h.score.toFixed(3)}</td>
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
  .note { font-size: 13px; color: var(--ink-secondary); margin: 4px 0 10px; }
  .ans { font-size: 14px; line-height: 1.6; color: var(--ink); margin: 6px 0 14px; white-space: pre-wrap; }
  .cite { color: var(--c-session, #2563eb); text-decoration: none; font-weight: 600; }
  .srclist { display: flex; flex-direction: column; gap: 10px; }
  .src { border-left: 2px solid var(--surface-alt, #ecebe4); padding-left: 10px; }
  .srchead { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 13px; }
  .ref { color: var(--ink); }
  .loc { color: var(--ink-muted); font-size: 12px; }
  .snip { color: var(--ink-secondary); font-size: 12px; margin-top: 3px; }
</style>
