<script lang="ts">
  import type { Observation, Memory } from '$lib/types';

  type Item =
    | { kind: 'observation'; data: Observation }
    | { kind: 'memory'; data: Memory }
    | null;

  let {
    item,
    onClose,
    onFilterToSession,
  }: {
    item: Item;
    onClose: () => void;
    onFilterToSession?: (sessionId: string) => void;
  } = $props();

  function fmt(ts?: string | null): string {
    if (!ts) return '—';
    try {
      return new Date(ts).toLocaleString();
    } catch {
      return ts;
    }
  }

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

  function sessionIdFromObs(obs: Observation): string {
    if (!obs.session_id) return '';
    return extractId(obs.session_id);
  }
</script>

{#if item}
  <aside class="drawer">
    <header class="drawer-head">
      {#if item.kind === 'observation'}
        <span class="badge badge-blue">{item.data.obs_type}</span>
      {:else}
        <span class="badge badge-purple">memory · {item.data.category}</span>
      {/if}
      <button type="button" class="close" onclick={onClose} aria-label="Close drawer">✕</button>
    </header>

    <h3 class="title">{item.data.title}</h3>

    {#if item.kind === 'observation'}
      {@const obs = item.data}
      <dl class="meta">
        <dt>When</dt>
        <dd>{fmt(obs.timestamp)}</dd>
        {#if obs.session_id}
          {@const sid = sessionIdFromObs(obs)}
          <dt>Session</dt>
          <dd>
            <a href={`/sessions/${encodeURIComponent(sid)}`}>{sid.slice(0, 12)}…</a>
            {#if onFilterToSession}
              <button type="button" class="link-btn" onclick={() => onFilterToSession(sid)}>
                filter to this →
              </button>
            {/if}
          </dd>
        {/if}
        <dt>Importance</dt>
        <dd>{obs.importance}{#if obs.confidence} · conf {obs.confidence}{/if}</dd>
      </dl>

      {#if obs.subtitle}
        <p class="subtitle">{obs.subtitle}</p>
      {/if}

      {#if obs.narrative}
        <section class="section">
          <h4 class="section-h">Narrative</h4>
          {#if obs.obs_type === 'command_run' || obs.obs_type === 'file_edit' || obs.obs_type === 'file_write'}
            <pre class="code"><code>{obs.narrative}</code></pre>
          {:else}
            <p class="narrative">{obs.narrative}</p>
          {/if}
        </section>
      {/if}

      {#if obs.facts && obs.facts.length}
        <section class="section">
          <h4 class="section-h">Facts</h4>
          <ul class="facts">
            {#each obs.facts as f}
              <li>{f}</li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if obs.files && obs.files.length}
        <section class="section">
          <h4 class="section-h">Files ({obs.files.length})</h4>
          <ul class="files">
            {#each obs.files as f}
              <li><code>{f}</code></li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if obs.keywords && obs.keywords.length}
        <section class="section">
          <h4 class="section-h">Keywords</h4>
          <div class="tags">
            {#each obs.keywords as k}
              <span class="badge badge-yellow">{k}</span>
            {/each}
          </div>
        </section>
      {/if}
    {:else}
      {@const mem = item.data}
      <dl class="meta">
        <dt>Project</dt>
        <dd>{mem.project}</dd>
        <dt>Updated</dt>
        <dd>{fmt(mem.updated_at)}</dd>
        <dt>Strength</dt>
        <dd>{mem.strength} · v{mem.version}</dd>
      </dl>

      {#if mem.content}
        <section class="section">
          <h4 class="section-h">Content</h4>
          <p class="narrative">{mem.content}</p>
        </section>
      {/if}

      {#if mem.keywords && mem.keywords.length}
        <section class="section">
          <h4 class="section-h">Keywords</h4>
          <div class="tags">
            {#each mem.keywords as k}
              <span class="badge badge-yellow">{k}</span>
            {/each}
          </div>
        </section>
      {/if}

      {#if mem.files && mem.files.length}
        <section class="section">
          <h4 class="section-h">Files</h4>
          <ul class="files">
            {#each mem.files as f}
              <li><code>{f}</code></li>
            {/each}
          </ul>
        </section>
      {/if}
    {/if}
  </aside>
{/if}

<style>
  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    height: 100vh;
    width: min(440px, 90vw);
    background: var(--bg);
    border-left: 1px solid var(--border);
    box-shadow: -4px 0 0 0 var(--border-light);
    z-index: 50;
    overflow-y: auto;
    padding: 16px 20px;
    font-family: var(--font-body);
  }

  .drawer-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border-light);
  }

  .close {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--ink-faint);
    font-size: 14px;
    padding: 4px 8px;
  }
  .close:hover { color: var(--accent); }

  .title {
    margin: 8px 0 12px;
    font-family: var(--font-display);
    font-size: 16px;
    font-weight: 700;
    line-height: 1.3;
  }

  .meta {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 4px 12px;
    margin: 0 0 16px;
    font-size: 11px;
    font-family: var(--font-ui);
  }
  .meta dt {
    color: var(--ink-faint);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-size: 9px;
    padding-top: 2px;
  }
  .meta dd { margin: 0; color: var(--ink); font-family: var(--font-mono); font-size: 10px; }
  .meta a { color: var(--accent); text-decoration: underline; }

  .link-btn {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--accent);
    font-size: 10px;
    padding: 0 0 0 8px;
    text-decoration: underline;
    font-family: var(--font-ui);
  }

  .subtitle {
    margin: 0 0 12px;
    color: var(--ink-muted);
    font-size: 12px;
    font-style: italic;
  }

  .section { margin-bottom: 14px; }

  .section-h {
    margin: 0 0 6px;
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-faint);
  }

  .narrative {
    margin: 0;
    font-size: 12px;
    line-height: 1.55;
    color: var(--ink-secondary);
  }

  .code {
    margin: 0;
    padding: 10px 12px;
    background: var(--bg-alt, #F0F0EC);
    border: 1px solid var(--border-light);
    font-family: var(--font-mono);
    font-size: 10px;
    line-height: 1.5;
    overflow-x: auto;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 360px;
  }

  .facts {
    margin: 0;
    padding-left: 18px;
    font-size: 11px;
    color: var(--ink-secondary);
    line-height: 1.5;
  }
  .facts li { margin-bottom: 3px; }

  .files {
    margin: 0;
    padding-left: 18px;
    font-size: 10px;
    line-height: 1.5;
  }
  .files code {
    font-family: var(--font-mono);
    color: var(--ink-secondary);
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
</style>
