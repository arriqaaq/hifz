<script lang="ts">
  import type { Observation } from '$lib/types';
  import { colorFor } from '$lib/components/graph/graphStyles';
  import type { CausalEdge } from '$lib/timeline/causality';

  let {
    observations,
    edges = [],
    selectedId = $bindable(),
    onSelect,
  }: {
    observations: Observation[];
    edges?: CausalEdge[];
    selectedId?: string;
    onSelect?: (obs: Observation | null) => void;
  } = $props();

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

  let LANE_ORDER = $derived.by(() => {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const o of observations) {
      if (!seen.has(o.obs_type)) {
        seen.add(o.obs_type);
        out.push(o.obs_type);
      }
    }
    // Stable preferred order first, then any extras
    const preferred = [
      'file_edit',
      'file_write',
      'command_run',
      'file_read',
      'search',
      'commit_made',
      'conversation',
      'other',
    ];
    return [...preferred.filter((t) => seen.has(t)), ...out.filter((t) => !preferred.includes(t))];
  });

  let bounds = $derived.by(() => {
    if (observations.length === 0) return { min: 0, max: 0, span: 1 };
    let min = Infinity,
      max = -Infinity;
    for (const o of observations) {
      const t = new Date(o.timestamp).getTime();
      if (t < min) min = t;
      if (t > max) max = t;
    }
    return { min, max, span: Math.max(1, max - min) };
  });

  const LANE_HEIGHT = 22;
  const TIME_AXIS_HEIGHT = 22;
  const PAD_X = 12;

  let plotWidth = $state(800);
  let containerEl: HTMLDivElement;

  $effect(() => {
    if (!containerEl) return;
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        plotWidth = Math.max(400, entry.contentRect.width - PAD_X * 2);
      }
    });
    ro.observe(containerEl);
    return () => ro.disconnect();
  });

  function xFor(ts: string): number {
    const t = new Date(ts).getTime();
    if (bounds.span === 0) return PAD_X;
    return PAD_X + ((t - bounds.min) / bounds.span) * plotWidth;
  }

  function yForLane(type: string): number {
    const idx = LANE_ORDER.indexOf(type);
    return TIME_AXIS_HEIGHT + idx * LANE_HEIGHT + LANE_HEIGHT / 2;
  }

  let plotHeight = $derived(TIME_AXIS_HEIGHT + LANE_ORDER.length * LANE_HEIGHT + 8);

  let ticks = $derived.by(() => {
    if (bounds.span <= 1) return [];
    const out: Array<{ x: number; label: string }> = [];
    const tickCount = 6;
    for (let i = 0; i <= tickCount; i++) {
      const t = bounds.min + (bounds.span * i) / tickCount;
      const d = new Date(t);
      const label = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
      out.push({ x: PAD_X + (i / tickCount) * plotWidth, label });
    }
    return out;
  });

  function handleClick(obs: Observation) {
    const id = extractId(obs.id);
    if (selectedId === id) {
      selectedId = undefined;
      onSelect?.(null);
    } else {
      selectedId = id;
      onSelect?.(obs);
    }
  }

  let obsById = $derived.by(() => {
    const m = new Map<string, Observation>();
    for (const o of observations) m.set(extractId(o.id), o);
    return m;
  });
</script>

<div class="waterfall" bind:this={containerEl}>
  <svg width="100%" height={plotHeight} class="plot" viewBox={`0 0 ${plotWidth + PAD_X * 2} ${plotHeight}`} preserveAspectRatio="none">
    <!-- Time axis -->
    <g class="time-axis">
      {#each ticks as t}
        <line x1={t.x} x2={t.x} y1={TIME_AXIS_HEIGHT - 4} y2={plotHeight - 4} class="tick-line" />
        <text x={t.x} y={14} class="tick-label" text-anchor="middle">{t.label}</text>
      {/each}
    </g>

    <!-- Lane labels & lines -->
    <g class="lanes">
      {#each LANE_ORDER as type, i}
        <line
          x1={PAD_X}
          x2={PAD_X + plotWidth}
          y1={TIME_AXIS_HEIGHT + i * LANE_HEIGHT + LANE_HEIGHT / 2}
          y2={TIME_AXIS_HEIGHT + i * LANE_HEIGHT + LANE_HEIGHT / 2}
          class="lane-line"
        />
      {/each}
    </g>

    <!-- Causal edges -->
    <g class="edges">
      {#each edges as edge}
        {@const a = obsById.get(edge.source)}
        {@const b = obsById.get(edge.target)}
        {#if a && b}
          {@const x1 = xFor(a.timestamp)}
          {@const y1 = yForLane(a.obs_type)}
          {@const x2 = xFor(b.timestamp)}
          {@const y2 = yForLane(b.obs_type)}
          <path
            d={`M${x1},${y1} C${(x1 + x2) / 2},${y1} ${(x1 + x2) / 2},${y2} ${x2},${y2}`}
            class={`edge edge--${edge.kind}`}
            fill="none"
          >
            <title>{edge.reason}</title>
          </path>
        {/if}
      {/each}
    </g>

    <!-- Observation dots -->
    <g class="obs-dots">
      {#each observations as obs}
        {@const id = extractId(obs.id)}
        {@const cx = xFor(obs.timestamp)}
        {@const cy = yForLane(obs.obs_type)}
        {@const r = obs.importance >= 7 ? 6 : 4}
        {@const isSel = selectedId === id}
        <circle
          {cx}
          {cy}
          r={isSel ? r + 2 : r}
          fill={colorFor(obs.obs_type)}
          fill-opacity={isSel ? 1 : 0.85}
          stroke={isSel ? '#1a1a1a' : 'none'}
          stroke-width="1.5"
          class="obs-dot"
          onclick={() => handleClick(obs)}
          onkeydown={(e: KeyboardEvent) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              handleClick(obs);
            }
          }}
          role="button"
          tabindex="0"
          aria-label={`${obs.obs_type}: ${obs.title}`}
        >
          <title>{obs.title}</title>
        </circle>
      {/each}
    </g>
  </svg>

  <div class="lane-labels" style={`top: ${TIME_AXIS_HEIGHT}px;`}>
    {#each LANE_ORDER as type}
      <div class="lane-label" style={`height: ${LANE_HEIGHT}px;`}>
        <span class="lane-dot" style={`background: ${colorFor(type)}`}></span>
        <span class="lane-name">{type}</span>
      </div>
    {/each}
  </div>

  <div class="legend">
    <span class="legend-item"><span class="legend-line legend-line--file"></span> shared file</span>
    <span class="legend-item"><span class="legend-line legend-line--keyword"></span> shared keywords</span>
    <span class="legend-item"><span class="legend-line legend-line--memory"></span> same memory</span>
  </div>
</div>

<style>
  .waterfall {
    position: relative;
    width: 100%;
    border: 1px solid var(--line-strong);
    background: var(--bg);
    padding: 8px 0 0 110px;
  }

  .plot {
    display: block;
    cursor: default;
  }

  .tick-line {
    stroke: var(--line);
    stroke-width: 0.5;
    stroke-dasharray: 2 3;
  }
  .tick-label {
    font-family: var(--font-mono);
    font-size: 10px;
    fill: var(--ink-faint);
  }

  .lane-line {
    stroke: var(--line);
    stroke-width: 0.5;
  }

  .edge {
    stroke-width: 1;
    pointer-events: stroke;
  }
  .edge--causal-file {
    stroke: #2D6A4F;
    stroke-opacity: 0.55;
    stroke-width: 1.5;
  }
  .edge--causal-keyword {
    stroke: #B8860B;
    stroke-opacity: 0.5;
    stroke-dasharray: 4 3;
  }
  .edge--causal-memory {
    stroke: #6B3FA0;
    stroke-opacity: 0.5;
    stroke-dasharray: 1 3;
  }

  .obs-dot {
    cursor: pointer;
    transition: r 120ms;
  }
  .obs-dot:hover {
    fill-opacity: 1 !important;
    stroke: var(--ink);
    stroke-width: 1;
  }

  .lane-labels {
    position: absolute;
    left: 0;
    width: 100px;
    padding-left: 6px;
  }
  .lane-label {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .lane-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .lane-name {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-secondary);
  }

  .legend {
    display: flex;
    gap: 16px;
    padding: 6px 12px;
    border-top: 1px solid var(--line);
    font-family: var(--font-ui);
    font-size: 10px;
    color: var(--ink-faint);
  }
  .legend-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .legend-line {
    display: inline-block;
    width: 24px;
    height: 0;
    border-top: 1.5px solid currentColor;
  }
  .legend-line--file { color: #2D6A4F; }
  .legend-line--keyword { color: #B8860B; border-top-style: dashed; }
  .legend-line--memory { color: #6B3FA0; border-top-style: dotted; border-top-width: 2px; }
</style>
