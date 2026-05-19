<!-- SPDX-License-Identifier: Apache-2.0 -->
<!-- Replay player. Sessions that have memory_delta observations (left);
     timed playback of their delta stream through DeltaView (right).
     Zero recompute — events already carry the structured MemoryDelta. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { getReplays, getReplay } from '$lib/api';
  import { loadTokens } from '$lib/tokens';
  import type { ReplaySession, SessionEvent, RenderTokens } from '$lib/types';
  import DeltaView from '$lib/components/DeltaView.svelte';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

  let sessions = $state<ReplaySession[]>([]);
  let tokens = $state<RenderTokens | null>(null);
  let active = $state<string | null>(null);
  let events = $state<SessionEvent[]>([]);
  let shown = $state(0);
  let playing = $state(false);
  let speed = $state(1);
  let loading = $state(true);
  let error = $state('');
  let timer: ReturnType<typeof setTimeout> | null = null;

  function clearTimer() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
  }

  onMount(async () => {
    tokens = await loadTokens();
    try {
      sessions = (await getReplays()).replays;
    } catch (e) {
      error = String(e);
    }
    loading = false;
    const q = page.url.searchParams.get('session');
    if (q) load(q);
  });

  $effect(() => () => clearTimer());

  async function load(sid: string) {
    clearTimer();
    playing = false;
    active = sid;
    shown = 0;
    try {
      events = (await getReplay(sid)).events;
      play();
    } catch (e) {
      error = String(e);
    }
  }

  function gap(i: number): number {
    if (i <= 0 || i >= events.length) return 700 / speed;
    const a = Date.parse(events[i - 1].t);
    const b = Date.parse(events[i].t);
    const d = isFinite(a) && isFinite(b) ? b - a : 700;
    return Math.min(Math.max(d, 250), 4000) / speed;
  }

  function tick() {
    if (shown >= events.length) {
      playing = false;
      return;
    }
    shown += 1;
    if (playing && shown < events.length) {
      timer = setTimeout(tick, gap(shown));
    } else {
      playing = false;
    }
  }

  function play() {
    if (events.length === 0) return;
    if (shown >= events.length) shown = 0;
    playing = true;
    clearTimer();
    timer = setTimeout(tick, 150);
  }
  function pause() {
    playing = false;
    clearTimer();
  }
  function step() {
    pause();
    if (shown < events.length) shown += 1;
  }
  function restart() {
    pause();
    shown = 0;
  }

  function ts(t: string): string {
    try {
      return new Date(t).toLocaleTimeString();
    } catch {
      return t;
    }
  }
</script>

<div class="wrap">
  <aside class="list">
    <h2>Replay</h2>
    {#if loading}
      <LoadingSpinner />
    {:else if sessions.length === 0}
      <p class="empty">No recorded sessions yet. Sessions with memory writes appear here.</p>
    {:else}
      {#each sessions as s}
        <button
          class="sess"
          class:active={active === s.session_id}
          onclick={() => load(s.session_id)}
        >
          <span class="sid">{s.session_id}</span>
          <span class="meta">{s.count} · {ts(s.last_ts)}</span>
        </button>
      {/each}
    {/if}
  </aside>

  <section class="player">
    {#if error}
      <p class="err">{error}</p>
    {/if}
    {#if !active}
      <p class="empty">Pick a session to replay its memory-state changes.</p>
    {:else}
      <header>
        <strong>{active}</strong>
        <span class="meta">{shown}/{events.length}</span>
      </header>
      <div class="transcript">
        {#each events.slice(0, shown) as ev}
          <div class="ev">
            <span class="evt">{ts(ev.t)}</span>
            <div class="evbody">
              {#if ev.kind === 'delta'}
                <DeltaView lines={ev.delta.lines} {tokens} />
              {:else if ev.kind === 'prompt'}
                <span class="prompt">▸ {ev.text}</span>
              {:else if ev.kind === 'note'}
                <span class="note">{ev.text}</span>
              {:else if ev.kind === 'error'}
                <span class="err">{ev.message}</span>
              {/if}
            </div>
          </div>
        {/each}
      </div>
      <footer>
        <button class="btn btn--small btn--accent" onclick={restart}>⏮ Restart</button>
        {#if playing}
          <button class="btn btn--small btn--accent" onclick={pause}>⏸ Pause</button>
        {:else}
          <button class="btn btn--small btn--accent" onclick={play}>▶ Play</button>
        {/if}
        <button class="btn btn--small btn--accent" onclick={step}>⏭ Step</button>
        <span class="spc">
          speed
          {#each [1, 2, 4] as sp}
            <button class="btn btn--small" class:btn--accent={speed === sp} onclick={() => (speed = sp)}>{sp}×</button>
          {/each}
        </span>
        <span class="bar"><span class="fill" style={`width:${events.length ? (shown / events.length) * 100 : 0}%`}></span></span>
      </footer>
    {/if}
  </section>
</div>

<style>
  .wrap {
    display: grid;
    grid-template-columns: 260px 1fr;
    gap: 16px;
    height: 100%;
    padding: 16px;
  }
  .list {
    border-right: 1px solid var(--line);
    padding-right: 12px;
    overflow-y: auto;
  }
  .list h2 {
    font-size: 14px;
    text-transform: uppercase;
    opacity: 0.7;
    margin: 0 0 10px;
  }
  .sess {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    text-align: left;
    background: none;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 8px;
    cursor: pointer;
    color: inherit;
  }
  .sess:hover {
    background: var(--surface-alt);
  }
  .sess.active {
    border-color: var(--ink);
  }
  .sid {
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }
  .meta {
    font-size: 11px;
    opacity: 0.6;
  }
  .player {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .player header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 8px;
  }
  .transcript {
    flex: 1;
    overflow-y: auto;
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 12px;
  }
  .ev {
    display: flex;
    gap: 10px;
    padding: 4px 0;
  }
  .evt {
    font-size: 11px;
    opacity: 0.5;
    flex: none;
    width: 72px;
    font-family: ui-monospace, monospace;
  }
  .evbody {
    min-width: 0;
  }
  .prompt {
    font-weight: 600;
  }
  .note {
    opacity: 0.7;
  }
  .err {
    color: var(--danger);
  }
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
  }
  .spc {
    display: flex;
    gap: 4px;
    align-items: center;
    font-size: 12px;
    opacity: 0.8;
  }
  .bar {
    flex: 1;
    height: 4px;
    background: var(--surface-alt);
    border-radius: 2px;
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--neon);
  }
  .empty {
    opacity: 0.6;
    font-style: italic;
  }
</style>
