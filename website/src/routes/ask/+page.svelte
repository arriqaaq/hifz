<script lang="ts">
  import {
    atlasAnswer,
    atlasQuery,
    getAtlasGraph,
    type AtlasAnswer,
    type AtlasHit,
    type AtlasGraph,
    type AtlasCitation,
  } from '$lib/api';
  import ProjectPicker from '$lib/components/atlas/ProjectPicker.svelte';
  import ConceptMap from '$lib/components/atlas/ConceptMap.svelte';
  import { ArrowRight, FileText, Code, Sparkles, File } from 'lucide-svelte';

  let project = $state('');
  let mode = $state<'ask' | 'search'>('ask');
  let q = $state('');

  let ans = $state<AtlasAnswer | null>(null);
  let hits = $state<AtlasHit[]>([]);
  let asking = $state(false);
  let searching = $state(false);
  let error = $state('');
  let copied = $state<string | null>(null);
  let facet = $state<string>('all');
  let expanded = $state<Set<string>>(new Set()); // search "similar results" toggles
  let passageOpen = $state<Set<string>>(new Set()); // source passage-pill toggles
  let moreOpen = $state<Set<number>>(new Set()); // "+N more" passage reveals

  // Concept map (B3): doc↔doc graph, shown only when a corpus is large enough.
  let graph = $state<AtlasGraph | null>(null);

  // Per-project chat history (single-turn Q&A), persisted in localStorage.
  type Chat = { id: number; q: string; ans: AtlasAnswer };
  let history = $state<Chat[]>([]);

  const STARTERS = ['Summarize this project', 'What are the key concepts?', 'How does it work?'];
  const PASSAGE_CAP = 6;

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
  function iconFor(kind?: string | null) {
    switch ((kind ?? '').toLowerCase()) {
      case 'code':
      case 'code_symbol':
        return Code;
      case 'concept':
        return Sparkles;
      case 'pdf':
      case 'file':
      case 'doc':
      case 'document':
        return FileText;
      default:
        return File;
    }
  }
  async function copyPath(uri: string) {
    try {
      await navigator.clipboard.writeText(uri);
      copied = uri;
      setTimeout(() => (copied = copied === uri ? null : copied), 1500);
    } catch {
      /* clipboard may be blocked */
    }
  }
  function toggleStr(set: Set<string>, key: string): Set<string> {
    const next = new Set(set);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  }

  async function runAnswer() {
    if (!q.trim() || !project) return;
    asking = true;
    hits = [];
    error = '';
    try {
      ans = await atlasAnswer(project, q.trim());
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      asking = false;
    }
  }
  async function runSearch() {
    if (!q.trim() || !project) return;
    searching = true;
    ans = null;
    error = '';
    facet = 'all';
    try {
      const r = await atlasQuery(project, q.trim());
      hits = r.hits ?? [];
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      searching = false;
    }
  }
  function run() {
    if (mode === 'ask') runAnswer();
    else runSearch();
  }

  // --- chat history ---------------------------------------------------------
  function historyKey(p: string) {
    return `ask.history.${p}`;
  }
  function loadHistory(p: string) {
    if (!p) {
      history = [];
      return;
    }
    try {
      history = JSON.parse(localStorage.getItem(historyKey(p)) || '[]');
    } catch {
      history = [];
    }
  }
  function saveHistory() {
    try {
      localStorage.setItem(historyKey(project), JSON.stringify(history.slice(0, 25)));
    } catch {
      /* non-browser / quota */
    }
  }
  function newChat() {
    if (ans && q.trim()) {
      history = [
        { id: Date.now(), q: q.trim(), ans },
        ...history.filter((h) => h.q !== q.trim()),
      ].slice(0, 25);
      saveHistory();
    }
    ans = null;
    hits = [];
    q = '';
    error = '';
    passageOpen = new Set();
    moreOpen = new Set();
  }
  function restore(c: Chat) {
    q = c.q;
    ans = c.ans;
    hits = [];
    mode = 'ask';
    error = '';
  }
  function relTime(id: number): string {
    const s = Math.max(1, Math.floor((Date.now() - id) / 1000));
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.floor(s / 60)}m ago`;
    if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
    return `${Math.floor(s / 86400)}d ago`;
  }

  function applyStarter(text: string) {
    q = text;
    mode = 'ask';
    runAnswer();
  }
  function applyFollowup(text: string) {
    q = text;
    mode = 'ask';
    runAnswer();
  }

  // Follow-up suggestions derived client-side from the cited documents.
  let followups = $derived(
    (ans?.citations ?? [])
      .slice(0, 3)
      .map((c) => `Tell me more about ${c.doc_label}`)
      .filter((v, i, a) => a.indexOf(v) === i),
  );

  // Search facets: counts by source kind (fallback to node kind).
  let facets = $derived(
    (() => {
      const m = new Map<string, number>();
      for (const h of hits) {
        const k = h.source_kind ?? h.kind ?? 'other';
        m.set(k, (m.get(k) ?? 0) + 1);
      }
      return [['all', hits.length] as [string, number], ...[...m.entries()].sort()];
    })(),
  );
  let shownHits = $derived(
    facet === 'all' ? hits : hits.filter((h) => (h.source_kind ?? h.kind ?? 'other') === facet),
  );

  let isEmpty = $derived(!ans && hits.length === 0 && !asking && !searching);
  let showMap = $derived(
    !!graph && (graph.nodes?.length ?? 0) >= 8 && (graph.edges?.length ?? 0) > 0,
  );

  async function onProjectChange() {
    ans = null;
    hits = [];
    q = '';
    error = '';
    graph = null;
    passageOpen = new Set();
    loadHistory(project);
    if (!project) return;
    try {
      graph = await getAtlasGraph(project, { docsOnly: true });
    } catch {
      graph = null;
    }
  }
</script>

{#snippet askbar()}
  <div class="askbar">
    <input
      class="qinput"
      placeholder={mode === 'ask' ? 'Ask a question about this project…' : 'Search the corpus…'}
      bind:value={q}
      onkeydown={(e) => e.key === 'Enter' && run()}
    />
    <button
      class="send"
      aria-label={mode === 'ask' ? 'Ask' : 'Search'}
      onclick={run}
      disabled={asking || searching || !q.trim()}
    >
      <ArrowRight size={18} strokeWidth={2} />
    </button>
  </div>
  <div class="modes">
    <button class="btn btn--small" class:btn--accent={mode === 'ask'} class:btn--ghost={mode !== 'ask'} onclick={() => (mode = 'ask')}>Ask</button>
    <button class="btn btn--small" class:btn--accent={mode === 'search'} class:btn--ghost={mode !== 'search'} onclick={() => (mode = 'search')}>Search</button>
  </div>
{/snippet}

{#snippet passages(c: AtlasCitation)}
  {#if (c.chunk_count ?? 1) > 1 && (c.chunks?.length ?? 0) > 1}
    {@const all = c.chunks ?? []}
    {@const visible = moreOpen.has(c.n) ? all : all.slice(0, PASSAGE_CAP)}
    <div class="passages">
      <div class="passages-label">{c.chunk_count} passages</div>
      <div class="loc-row">
        {#each visible as ch, i}
          {@const key = `${c.n}:${i}`}
          <button
            class="loc-pill"
            class:active={passageOpen.has(key)}
            onclick={() => (passageOpen = toggleStr(passageOpen, key))}
          >
            {ch.location ?? `chunk ${i}`}
          </button>
        {/each}
        {#if all.length > PASSAGE_CAP && !moreOpen.has(c.n)}
          <button class="loc-pill more" onclick={() => (moreOpen = new Set(moreOpen).add(c.n))}>
            +{all.length - PASSAGE_CAP} more
          </button>
        {/if}
      </div>
      {#each visible as ch, i}
        {#if passageOpen.has(`${c.n}:${i}`) && ch.snippet}
          <div class="passage-body"><span class="ploc">{ch.location ?? `chunk ${i}`}</span>{ch.snippet}</div>
        {/if}
      {/each}
    </div>
  {/if}
{/snippet}

<div class="page">
  <div class="topbar">
    <ProjectPicker bind:project onchange={onProjectChange} />
  </div>

  {#if !project}
    <div class="card empty">
      <div class="card-title">No project selected</div>
      <p class="muted">Create or select a project above to ask its corpus.</p>
    </div>
  {:else if isEmpty}
    <section class="hero">
      <h1 class="hero-h">Ask <span class="proj">{project}</span></h1>
      <p class="muted hero-sub">Search across this project's knowledge — documents, code, and concepts.</p>
      {@render askbar()}
      {#if error}<div class="errline"><span class="badge badge-red">error</span> {error}</div>{/if}

      {#if mode === 'ask'}
        <div class="starters">
          <span class="try">Try</span>
          {#each STARTERS as s}
            <button class="starter" onclick={() => applyStarter(s)}>{s}</button>
          {/each}
        </div>
      {:else}
        <p class="muted hint">Search returns one entry per document, with matching passages folded in.</p>
      {/if}

      {#if history.length}
        <div class="recent">
          <div class="recent-h">Recent</div>
          {#each history as c}
            <button class="recent-item" onclick={() => restore(c)}>
              <span class="rq">{c.q}</span>
              <span class="rt">{relTime(c.id)}</span>
            </button>
          {/each}
        </div>
      {/if}

      {#if showMap}
        <div class="card map-card">
          <div class="card-title">Concept map — related documents</div>
          <ConceptMap {graph} />
        </div>
      {/if}
    </section>
  {:else}
    <div class="asktop">
      {@render askbar()}
    </div>
    {#if error}<div class="errline"><span class="badge badge-red">error</span> {error}</div>{/if}

    {#if mode === 'search'}
      <div class="filterchips">
        {#each facets as [k, n]}
          <button class="chip" class:btn--accent={facet === k} onclick={() => (facet = k)}>
            <span class="ck">{k}</span> <span class="cn">{n}</span>
          </button>
        {/each}
      </div>
    {/if}

    <div class="layout">
      <div class="main">
        {#if mode === 'ask'}
          {#if asking}
            <p class="muted">Thinking…</p>
          {:else if ans}
            {#if ans.note}
              <div class="note"><span class="badge badge-amber">note</span> {ans.note}</div>
            {/if}
            {#if ans.answer}
              <p class="ans">
                {#each segments(ans.answer) as seg}
                  {#if seg.t === 'text'}{seg.v}{:else}<a class="cite" href={`#src-${seg.n}`}>[{seg.n}]</a>{/if}
                {/each}
              </p>
            {/if}
            {#if followups.length}
              <div class="related">
                <span class="try">Related</span>
                {#each followups as f}
                  <button class="starter" onclick={() => applyFollowup(f)}>{f}</button>
                {/each}
              </div>
            {/if}
            {#if showMap}
              <div class="card map-card">
                <div class="card-title">Concept map — related documents</div>
                <ConceptMap {graph} />
              </div>
            {/if}
          {/if}
        {:else if searching}
          <p class="muted">Searching…</p>
        {:else}
          <div class="results">
            {#each shownHits as h}
              {@const Icon = iconFor(h.source_kind ?? h.kind)}
              <div class="result">
                <div class="rhead">
                  <span class="dtype"><Icon size={13} strokeWidth={1.8} /> {h.source_kind ?? h.kind}</span>
                  {#if isLink(h.source_uri)}
                    <a class="rtitle" href={h.source_uri} target="_blank" rel="noreferrer">{h.doc_label}</a>
                  {:else}
                    <span class="rtitle">{h.doc_label}</span>
                  {/if}
                  <span class="rscore">{h.score.toFixed(3)}</span>
                </div>
                {#if h.source_ref && h.source_ref !== h.doc_label}
                  <div class="rref mono">{h.source_ref}</div>
                {/if}
                {#if h.snippet}<div class="snip">{h.snippet}</div>{/if}
                {#if (h.chunk_count ?? 1) > 1}
                  <button class="btn btn--small similar" onclick={() => (expanded = toggleStr(expanded, h.id))}>
                    {expanded.has(h.id) ? 'Hide passages' : 'Similar results'} ({(h.chunk_count ?? 1) - 1})
                  </button>
                  {#if expanded.has(h.id)}
                    <div class="chunks">
                      {#each (h.chunks ?? []).slice(1) as ch}
                        <div class="chunk">
                          {#if ch.location}<span class="ploc">{ch.location}</span>{/if}
                          {#if ch.snippet}<span class="snip">{ch.snippet}</span>{/if}
                        </div>
                      {/each}
                    </div>
                  {/if}
                {/if}
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <aside class="side">
        {#if mode === 'ask'}
          <div class="side-head">
            <span class="card-title">Sources</span>
            <button class="btn btn--small" onclick={newChat}>New chat</button>
          </div>
          {#if ans?.citations?.length}
            <div class="srclist">
              {#each ans.citations as c}
                {@const Icon = iconFor(c.source_kind)}
                <div class="srccard" id={`src-${c.n}`}>
                  <div class="srchead">
                    <span class="cite">[{c.n}]</span>
                    <span class="dtype"><Icon size={13} strokeWidth={1.8} /> {c.source_kind ?? 'doc'}</span>
                    {#if isLink(c.source_uri)}
                      <a class="ref" href={c.source_uri} target="_blank" rel="noreferrer">{c.source_ref ?? c.doc_label}</a>
                    {:else}
                      <span class="ref mono">{c.source_ref ?? c.doc_label}</span>
                      {#if c.source_uri}
                        <button class="btn btn--small" onclick={() => copyPath(c.source_uri!)}>
                          {copied === c.source_uri ? 'copied' : 'copy path'}
                        </button>
                      {/if}
                    {/if}
                  </div>
                  {#if c.snippet}<div class="snip">{c.snippet}</div>{/if}
                  {@render passages(c)}
                </div>
              {/each}
            </div>
          {:else}
            <div class="muted side-empty">Sources appear here, one per document.</div>
          {/if}
        {:else}
          <div class="side-head">
            <span class="card-title">Filter by type</span>
            <button class="btn btn--small" onclick={() => (mode = 'ask')}>New answer</button>
          </div>
          <div class="facetlist">
            {#each facets as [k, n]}
              <button class="facet" class:active={facet === k} onclick={() => (facet = k)}>
                <span class="fk">{k}</span>
                <span class="fn">{n}</span>
              </button>
            {/each}
          </div>
        {/if}
      </aside>
    </div>
  {/if}
</div>

<style>
  .page { padding: 24px; max-width: 1180px; margin: 0 auto; }
  .topbar { display: flex; justify-content: flex-end; margin-bottom: 8px; }
  .muted { color: var(--ink-muted); }

  /* hero / empty */
  .hero { max-width: 720px; margin: 0 auto; padding-top: 8vh; text-align: center; }
  .hero-h { font-family: var(--font-display); font-size: 30px; font-weight: 700; margin: 0 0 6px; }
  .hero-h .proj { color: var(--ink-muted); }
  .hero-sub { font-size: 14px; margin: 0 0 20px; }
  .hero .askbar, .hero .modes, .hero .starters, .hero .recent { text-align: left; }

  /* ask bar — hifz brand: ink border + hard offset shadow */
  .askbar {
    display: flex; align-items: center; gap: 8px;
    background: var(--surface);
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
    padding: 6px 6px 6px 14px;
  }
  .qinput { flex: 1; border: none; background: transparent; font-size: 16px; padding: 10px 0; outline: none; color: var(--ink); }
  .send {
    flex: none; width: 38px; height: 38px; border-radius: 999px;
    background: var(--neon); color: var(--ink); border: 1px solid var(--line-strong);
    display: flex; align-items: center; justify-content: center; cursor: pointer;
    transition: filter 120ms;
  }
  .send:hover:not(:disabled) { filter: brightness(0.95); }
  .send:disabled { opacity: 0.45; cursor: default; }
  .modes { display: flex; gap: 6px; margin: 10px 0 0; }
  .asktop { margin-bottom: 16px; }

  /* starter + related pills */
  .starters, .related { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; margin-top: 18px; }
  .try { font-size: 12px; color: var(--ink-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .starter {
    border: 1px solid var(--line-strong); background: var(--surface-alt); color: var(--ink);
    border-radius: 999px; padding: 6px 14px; font-size: 13px; cursor: pointer; transition: background 120ms;
  }
  .starter:hover { background: var(--neon-dim); }
  .hint { margin-top: 16px; font-size: 13px; }
  .errline { margin: 12px 0; font-size: 13px; }

  /* recent history */
  .recent { margin-top: 26px; }
  .recent-h { font-size: 12px; color: var(--ink-muted); text-transform: uppercase; letter-spacing: 0.04em; margin-bottom: 6px; }
  .recent-item {
    display: flex; align-items: baseline; justify-content: space-between; gap: 12px; width: 100%;
    background: transparent; border: none; border-bottom: 1px solid var(--line); padding: 9px 2px;
    cursor: pointer; text-align: left; color: var(--ink); font-size: 13px;
  }
  .recent-item:hover { background: var(--surface-alt); }
  .recent-item .rq { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .recent-item .rt { color: var(--ink-faint); font-size: 12px; flex: none; }

  /* answer / search two-column */
  .layout { display: grid; grid-template-columns: 1fr var(--drawer-w); gap: 20px; align-items: start; }
  @media (max-width: 860px) { .layout { grid-template-columns: 1fr; } }
  .note { font-size: 13px; color: var(--ink-secondary); margin: 0 0 10px; }
  .ans { font-size: 15px; line-height: 1.7; color: var(--ink); margin: 4px 0 16px; white-space: pre-wrap; }
  .cite {
    color: var(--ink); text-decoration: none; font-weight: 600; font-size: 12px;
    border: 1px solid var(--line-strong); border-radius: var(--radius-sm); padding: 0 4px; margin: 0 1px;
  }
  .cite:hover { background: var(--neon-dim); }

  /* search filter chips */
  .filterchips { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 16px; }
  .chip {
    border: 1px solid var(--line-strong); background: var(--surface); color: var(--ink);
    border-radius: 999px; padding: 5px 12px; font-size: 13px; cursor: pointer; text-transform: capitalize;
  }
  .chip .cn { color: var(--ink-faint); }
  .chip.btn--accent { background: var(--neon); }
  .chip.btn--accent .cn { color: var(--ink); }

  /* search results */
  .results { display: flex; flex-direction: column; gap: 14px; }
  .result { border-bottom: 1px solid var(--line); padding-bottom: 12px; }
  .rhead { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .rtitle { font-weight: 600; color: var(--blue); text-decoration: none; }
  .rtitle:hover { text-decoration: underline; }
  .rscore { margin-left: auto; color: var(--ink-faint); font-size: 12px; }
  .rref { color: var(--ink-muted); font-size: 12px; margin-top: 2px; }
  .dtype { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.03em; color: var(--ink-muted); }
  .snip { color: var(--ink-secondary); font-size: 13px; margin-top: 4px; line-height: 1.5; }
  .similar { margin-top: 8px; }
  .chunks { display: flex; flex-direction: column; gap: 6px; margin-top: 8px; padding-left: 10px; border-left: 2px solid var(--surface-alt); }
  .chunk { font-size: 12px; }

  /* sources sidebar */
  .side-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 12px; }
  .srclist { display: flex; flex-direction: column; gap: 12px; }
  .srccard {
    background: var(--surface); border: 1px solid var(--line-strong);
    border-radius: var(--radius); box-shadow: var(--shadow-sm); padding: 10px 12px;
  }
  .srchead { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 13px; }
  .cite + .dtype { margin-left: 0; }
  .ref { color: var(--ink); }
  .side-empty { padding: 4px 2px; font-size: 13px; }

  /* passages */
  .passages { margin-top: 8px; }
  .passages-label { font-size: 11px; color: var(--ink-muted); text-transform: uppercase; letter-spacing: 0.03em; margin-bottom: 5px; }
  .loc-row { display: flex; flex-wrap: wrap; gap: 5px; }
  .loc-pill {
    border: 1px solid var(--line-strong); background: var(--surface-alt); color: var(--ink);
    border-radius: 999px; padding: 2px 9px; font-size: 11px; cursor: pointer; font-family: var(--font-mono);
  }
  .loc-pill:hover { background: var(--neon-dim); }
  .loc-pill.active { background: var(--neon); }
  .loc-pill.more { font-family: var(--font-ui); color: var(--ink-muted); }
  .passage-body { font-size: 12px; color: var(--ink-secondary); margin-top: 6px; line-height: 1.5; }
  .ploc { font-family: var(--font-mono); font-size: 11px; color: var(--ink-muted); margin-right: 6px; }

  /* facet sidebar (search) */
  .facetlist { display: flex; flex-direction: column; gap: 2px; }
  .facet { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 7px 10px; border-radius: var(--radius-sm); background: transparent; border: none; cursor: pointer; color: var(--ink-secondary); font-size: 13px; text-align: left; text-transform: capitalize; }
  .facet:hover { background: var(--surface-alt); }
  .facet.active { background: var(--surface-alt); color: var(--ink); font-weight: 600; }
  .fn { color: var(--ink-faint); }

  .map-card { margin-top: 16px; }
  .empty .muted { margin: 6px 0 0; }
</style>
