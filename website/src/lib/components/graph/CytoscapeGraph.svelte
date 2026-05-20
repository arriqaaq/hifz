<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import cytoscape, { type Core, type ElementDefinition, type LayoutOptions } from 'cytoscape';
  // @ts-expect-error — no types for fcose
  import fcose from 'cytoscape-fcose';
  import { stylesheet, colorFor } from './graphStyles';

  cytoscape.use(fcose as never);

  export interface GraphInputNode {
    id: string;
    label: string;
    type: string;
    kind:
      | 'memory'
      | 'observation'
      | 'session'
      | 'run'
      | 'commit'
      | 'document'
      | 'concept'
      | 'code_symbol'
      | 'external'
      | 'file';
    keywords?: string[];
    files?: string[];
    timestamp?: string;
    obs_type?: string;
    importance?: number;
    raw?: unknown;
  }

  export type EdgeClass =
    | 'shared-keyword'
    | 'causal-file'
    | 'causal-keyword'
    | 'causal-memory';
  export type EdgeRel =
    | 'IN_SESSION'
    | 'IN_RUN'
    | 'SHARES_FILE'
    | 'RECALLS'
    | 'DISTILLED_FROM'
    | 'PRODUCED_BY';

  export interface GraphInputEdge {
    source: string;
    target: string;
    kind?: EdgeClass;
    // hifz activity rels (EdgeRel) or maktab relations
    // (calls/imports/contains/related/mentions, …) — accept any string.
    rel?: EdgeRel | string;
  }

  let {
    nodes,
    edges,
    selectedId = $bindable(),
    onSelect,
    onExpand,
    compact = false,
  }: {
    nodes: GraphInputNode[];
    edges: GraphInputEdge[];
    selectedId?: string;
    onSelect?: (node: GraphInputNode | null) => void;
    onExpand?: (node: GraphInputNode) => void;
    compact?: boolean;
  } = $props();

  let container = $state<HTMLDivElement | null>(null);
  let cy: Core | null = null;
  let layoutName = $state<'fcose' | 'concentric' | 'breadthfirst' | 'circle'>('fcose');
  let pinnedCount = $state(0);
  let labelMode = $state<'all' | 'hover' | 'selected'>('hover');

  function buildElements(ns: GraphInputNode[], es: GraphInputEdge[]): ElementDefinition[] {
    const els: ElementDefinition[] = [];
    const degree = new Map<string, number>();
    for (const e of es) {
      degree.set(e.source, (degree.get(e.source) ?? 0) + 1);
      degree.set(e.target, (degree.get(e.target) ?? 0) + 1);
    }
    const nodeIds = new Set<string>();
    for (const n of ns) {
      const deg = degree.get(n.id) ?? 0;
      const baseSize = n.kind === 'memory' ? 26 : 14;
      const size = Math.max(baseSize, Math.min(48, baseSize + deg * 2));
      nodeIds.add(n.id);
      els.push({
        data: {
          id: n.id,
          label: n.label.length > 40 ? n.label.slice(0, 38) + '…' : n.label,
          fullLabel: n.label,
          type: n.type,
          kind: n.kind,
          color: colorFor(n.type),
          size,
          raw: n,
        },
      });
    }
    for (const e of es) {
      // Cytoscape throws on an edge whose source/target node is absent, which
      // blanks the whole canvas — skip orphans defensively (the backend also
      // prunes them, but maktab graphs can exceed the node cap and dangle).
      if (!nodeIds.has(e.source) || !nodeIds.has(e.target)) continue;
      els.push({
        data: {
          id: `${e.source}__${e.target}`,
          source: e.source,
          target: e.target,
          rel: e.rel ?? '',
        },
        classes: e.kind ?? '',
      });
    }
    return els;
  }

  function layoutOpts(name: string): LayoutOptions {
    // Large graphs (corpus/activity exports run into the thousands of nodes)
    // freeze the tab if we animate a 2500-iteration force layout — every
    // iteration repaints. Above the threshold, compute the layout once
    // (animate off) with cheaper settings.
    const big = nodes.length > 150;
    if (name === 'fcose') {
      return {
        name: 'fcose',
        quality: big ? 'draft' : 'default',
        animate: !big,
        animationDuration: 600,
        randomize: big,
        nodeSeparation: 80,
        idealEdgeLength: 100,
        nodeRepulsion: () => 6000,
        gravity: 0.25,
        gravityRange: 3.5,
        numIter: big ? 500 : 2500,
        tile: !big,
        packComponents: !big,
      } as unknown as LayoutOptions;
    }
    if (name === 'concentric') {
      return {
        name: 'concentric',
        animate: !big,
        animationDuration: 500,
        concentric: (n) => {
          const raw = (n.data('raw') as GraphInputNode | undefined) ?? null;
          return (raw?.importance ?? 0) + (n.degree(false) ?? 0);
        },
        levelWidth: () => 1,
      };
    }
    if (name === 'breadthfirst') {
      return { name: 'breadthfirst', animate: !big, animationDuration: 500, spacingFactor: 1.4 };
    }
    return { name: 'circle', animate: !big };
  }

  function applyLabelMode() {
    if (!cy) return;
    cy.nodes().removeClass('show-label');
    if (labelMode === 'all') cy.nodes().addClass('show-label');
  }

  onMount(() => {
    if (!container) return;
    if (compact) labelMode = 'selected';
    cy = cytoscape({
      container,
      elements: buildElements(nodes, edges),
      style: stylesheet,
      layout: layoutOpts('fcose'),
      wheelSensitivity: 0.2,
      minZoom: 0.1,
      maxZoom: 4,
      boxSelectionEnabled: false,
      autoungrabify: false,
    });

    applyLabelMode();

    cy.on('dragfree', 'node', (evt) => {
      const node = evt.target;
      node.lock();
      node.addClass('locked');
      pinnedCount = cy?.nodes('.locked').length ?? 0;
    });

    cy.on('tap', 'node', (evt) => {
      const node = evt.target;
      const raw = node.data('raw') as GraphInputNode | undefined;
      selectedId = node.id();
      onSelect?.(raw ?? null);
    });

    cy.on('tap', (evt) => {
      // Tap on background clears selection.
      if (evt.target === cy) {
        selectedId = undefined;
        onSelect?.(null);
      }
    });

    cy.on('dbltap', 'node', (evt) => {
      const node = evt.target;
      const raw = node.data('raw') as GraphInputNode | undefined;
      if (raw) onExpand?.(raw);
    });

    if (labelMode === 'hover') {
      cy.on('mouseover', 'node', (evt) => evt.target.addClass('show-label'));
      cy.on('mouseout', 'node', (evt) => evt.target.removeClass('show-label'));
    }

    // Neighborhood dim: on hover, fade everything outside the 1-hop neighborhood.
    cy.on('mouseover', 'node', (evt) => {
      if (!cy) return;
      const node = evt.target;
      const keep = node.closedNeighborhood();
      cy.elements().not(keep).addClass('faded');
      keep.connectedEdges().addClass('show-label');
    });
    cy.on('mouseout', 'node', () => {
      if (!cy) return;
      cy.elements().removeClass('faded');
      cy.edges().removeClass('show-label');
    });
  });

  // Re-render when input changes.
  $effect(() => {
    if (!cy) return;
    cy.batch(() => {
      cy!.elements().remove();
      cy!.add(buildElements(nodes, edges));
    });
    cy.layout(layoutOpts(layoutName)).run();
    applyLabelMode();
    pinnedCount = 0;
  });

  // Highlight selection from outside.
  $effect(() => {
    if (!cy) return;
    cy.nodes().unselect();
    if (selectedId) {
      const n = cy.getElementById(selectedId);
      if (n.nonempty()) n.select();
    }
  });

  function relayout() {
    if (!cy) return;
    cy.nodes().unlock();
    cy.nodes().removeClass('locked');
    pinnedCount = 0;
    cy.layout(layoutOpts(layoutName)).run();
  }

  function unpinAll() {
    if (!cy) return;
    cy.nodes().unlock();
    cy.nodes().removeClass('locked');
    pinnedCount = 0;
  }

  function fit() {
    cy?.fit(undefined, 50);
  }

  function setLayout(name: typeof layoutName) {
    layoutName = name;
    if (!cy) return;
    cy.nodes().unlock();
    cy.nodes().removeClass('locked');
    pinnedCount = 0;
    cy.layout(layoutOpts(name)).run();
  }

  function cycleLabels() {
    labelMode = labelMode === 'hover' ? 'all' : labelMode === 'all' ? 'selected' : 'hover';
    applyLabelMode();
  }

  onDestroy(() => {
    cy?.destroy();
    cy = null;
  });
</script>

<div class="graph-wrap" class:compact>
  <div class="cy-container" bind:this={container}></div>

  {#if !compact}
    <div class="controls">
      <div class="ctrl-group">
        <span class="ctrl-label">Layout</span>
        <button class="ctrl-btn" class:active={layoutName === 'fcose'} onclick={() => setLayout('fcose')}>fcose</button>
        <button class="ctrl-btn" class:active={layoutName === 'concentric'} onclick={() => setLayout('concentric')}>concentric</button>
        <button class="ctrl-btn" class:active={layoutName === 'breadthfirst'} onclick={() => setLayout('breadthfirst')}>tree</button>
        <button class="ctrl-btn" class:active={layoutName === 'circle'} onclick={() => setLayout('circle')}>circle</button>
      </div>
      <div class="ctrl-group">
        <button class="ctrl-btn" onclick={relayout}>↻ relayout</button>
        <button class="ctrl-btn" onclick={fit}>⛶ fit</button>
        <button class="ctrl-btn" onclick={cycleLabels}>labels: {labelMode}</button>
        {#if pinnedCount > 0}
          <button class="ctrl-btn ctrl-btn--accent" onclick={unpinAll}>unpin {pinnedCount}</button>
        {/if}
      </div>
    </div>

    <div class="hint">
      Drag a node to pin it · Click for details · Double-click to expand · Scroll to zoom
    </div>
  {/if}
</div>

<style>
  .graph-wrap {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--bg);
  }
  .cy-container {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }

  .controls {
    position: absolute;
    bottom: 12px;
    left: 12px;
    z-index: 2;
    display: flex;
    flex-direction: column;
    gap: 6px;
    background: var(--surface);
    border: 1.5px solid var(--ink);
    border-radius: var(--radius-sm);
    box-shadow: 4px 4px 0 var(--ink);
    padding: 10px 12px;
  }
  .ctrl-group {
    display: flex;
    gap: 4px;
    align-items: center;
  }
  .ctrl-label {
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-faint);
    margin-right: 4px;
  }
  .ctrl-btn {
    padding: 4px 10px;
    font-size: 10px;
    font-weight: 500;
    font-family: var(--font-ui);
    background: var(--bg);
    border: 1px solid var(--line-strong);
    color: var(--ink);
    cursor: pointer;
  }
  .ctrl-btn:hover { background: var(--surface-alt); }
  .ctrl-btn.active {
    background: var(--ink);
    color: var(--bg);
    border-color: var(--ink);
  }
  .ctrl-btn--accent {
    background: var(--neon);
    color: var(--ink);
    border-color: var(--ink);
    box-shadow: 2px 2px 0 var(--ink);
    font-weight: 600;
  }
  .ctrl-btn--accent:hover {
    background: var(--neon);
    transform: translate(-1px, -1px);
    box-shadow: 3px 3px 0 var(--ink);
  }

  .hint {
    position: absolute;
    bottom: 12px;
    right: 12px;
    z-index: 2;
    padding: 6px 10px;
    background: var(--surface);
    border: 1.5px solid var(--ink);
    border-radius: var(--radius-sm);
    box-shadow: 2px 2px 0 var(--ink);
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-muted);
    letter-spacing: 0.04em;
  }
</style>
