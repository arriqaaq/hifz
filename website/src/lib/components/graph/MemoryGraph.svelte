<script lang="ts">
  import { onMount } from 'svelte';

  interface GraphNode {
    id: string;
    x: number;
    y: number;
    vx: number;
    vy: number;
    r: number;
    label: string;
    type: string;
    kind: 'memory' | 'observation';
  }

  interface GraphEdge {
    source: number;
    target: number;
  }

  let { nodes, edges }: { nodes: GraphNode[]; edges: GraphEdge[] } = $props();

  let canvas: HTMLCanvasElement;
  let running = $state(true);
  let hovered = $state<GraphNode | null>(null);
  let dragging = $state<GraphNode | null>(null);
  let nodeCount = $derived(nodes.length);
  let edgeCount = $derived(edges.length);

  const TYPE_COLORS: Record<string, string> = {
    // Memory types
    architecture: '#B8860B',
    pattern: '#2563EB',
    fact: '#2D6A4F',
    workflow: '#1D4E89',
    bug: '#CC0000',
    preference: '#6B3FA0',
    // Observation types (actual values from hifz)
    command_run: '#0E7490',
    file_edit: '#C2410C',
    file_read: '#2D6A4F',
    file_write: '#B8860B',
    conversation: '#6B3FA0',
    search: '#2563EB',
    other: '#999999',
  };

  function getColor(type: string): string {
    return TYPE_COLORS[type] ?? '#666666';
  }

  onMount(() => {
    const ctx = canvas.getContext('2d')!;
    const dpr = window.devicePixelRatio || 1;
    let raf: number;

    let panX = 0, panY = 0, zoom = 1;
    let isPanning = false;
    let panStartX = 0, panStartY = 0;
    let dragOffsetX = 0, dragOffsetY = 0;

    function resize() {
      const rect = canvas.parentElement!.getBoundingClientRect();
      canvas.width = rect.width * dpr;
      canvas.height = rect.height * dpr;
      canvas.style.width = `${rect.width}px`;
      canvas.style.height = `${rect.height}px`;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

      // No pan offset needed — nodes are positioned at canvas center
    }

    resize();
    window.addEventListener('resize', resize);

    // Initial positioning: circle centered on canvas, radius relative to canvas size
    const rect0 = canvas.parentElement!.getBoundingClientRect();
    const canvasW = rect0.width;
    const canvasH = rect0.height;
    const cx = canvasW / 2;
    const cy = canvasH / 2;
    const initRadius = Math.min(canvasW, canvasH) * 0.3;

    for (let i = 0; i < nodes.length; i++) {
      if (nodes[i].x === 0 && nodes[i].y === 0) {
        const angle = (2 * Math.PI * i) / nodes.length;
        nodes[i].x = cx + initRadius * Math.cos(angle);
        nodes[i].y = cy + initRadius * Math.sin(angle);
        nodes[i].vx = 0;
        nodes[i].vy = 0;
      }
    }

    // Force simulation parameters (agentmemory exact)
    const damping = 0.9;
    const n = nodes.length;
    const repulsion = n > 100 ? 2000 : n > 50 ? 1200 : 800;
    const attraction = n > 100 ? 0.002 : 0.005;
    const restDist = 100;
    const centerGravity = n > 100 ? 0.005 : 0.01;

    function simulate() {
      for (let i = 0; i < n; i++) {
        if (nodes[i] === dragging) continue;
        let fx = 0, fy = 0;

        // N² repulsion
        for (let j = 0; j < n; j++) {
          if (i === j) continue;
          const dx = nodes[i].x - nodes[j].x;
          const dy = nodes[i].y - nodes[j].y;
          const distSq = dx * dx + dy * dy + 1;
          const f = repulsion / distSq;
          fx += dx * f / Math.sqrt(distSq);
          fy += dy * f / Math.sqrt(distSq);
        }

        // Center gravity
        fx += (cx - nodes[i].x) * centerGravity;
        fy += (cy - nodes[i].y) * centerGravity;

        nodes[i].vx = (nodes[i].vx + fx) * damping;
        nodes[i].vy = (nodes[i].vy + fy) * damping;
      }

      // Edge attraction (separate pass)
      for (const e of edges) {
        const a = nodes[e.source];
        const b = nodes[e.target];
        if (!a || !b || a === dragging || b === dragging) continue;
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const dist = Math.sqrt(dx * dx + dy * dy) + 0.1;
        const f = (dist - restDist) * attraction;
        const efx = (dx / dist) * f;
        const efy = (dy / dist) * f;
        if (a !== dragging) { a.vx += efx; a.vy += efy; }
        if (b !== dragging) { b.vx -= efx; b.vy -= efy; }
      }

      // Apply velocity
      for (const nd of nodes) {
        if (nd === dragging) continue;
        nd.x += nd.vx;
        nd.y += nd.vy;
      }
    }

    function draw() {
      if (!running) { raf = requestAnimationFrame(draw); return; }

      simulate();

      const w = canvas.width / dpr;
      const h = canvas.height / dpr;

      ctx.save();
      ctx.clearRect(0, 0, w, h);

      // Background
      ctx.fillStyle = '#F9F9F7';
      ctx.fillRect(0, 0, w, h);

      // Grid dots
      ctx.fillStyle = '#D4D4CF';
      const gridSize = 40;
      const startX = -(panX * zoom % gridSize);
      const startY = -(panY * zoom % gridSize);
      for (let x = startX; x < w; x += gridSize) {
        for (let y = startY; y < h; y += gridSize) {
          ctx.fillRect(x, y, 1, 1);
        }
      }

      // Apply pan + zoom transform
      ctx.translate(panX, panY);
      ctx.scale(zoom, zoom);

      // Edges
      for (const e of edges) {
        const a = nodes[e.source];
        const b = nodes[e.target];
        if (!a || !b) continue;
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const dist = Math.sqrt(dx * dx + dy * dy);
        const alpha = Math.max(0.05, Math.min(0.3, 1 - dist / 500));

        ctx.beginPath();
        // Curved edge
        const mx = (a.x + b.x) / 2;
        const my = (a.y + b.y) / 2;
        const offset = Math.min(20, dist * 0.08);
        const nx = -(b.y - a.y) / dist * offset;
        const ny = (b.x - a.x) / dist * offset;
        ctx.moveTo(a.x, a.y);
        ctx.quadraticCurveTo(mx + nx, my + ny, b.x, b.y);
        ctx.strokeStyle = `rgba(180, 180, 170, ${alpha})`;
        ctx.lineWidth = 0.5;
        ctx.stroke();
      }

      // Nodes
      const showLabels = n <= 40;
      for (const nd of nodes) {
        const col = getColor(nd.type);
        const isHov = nd === hovered;

        // Glow on hover
        if (isHov) {
          ctx.beginPath();
          ctx.arc(nd.x, nd.y, nd.r + 6, 0, Math.PI * 2);
          ctx.fillStyle = col + '22';
          ctx.fill();
        }

        // Node fill
        ctx.beginPath();
        ctx.arc(nd.x, nd.y, nd.r, 0, Math.PI * 2);
        ctx.fillStyle = isHov ? col : col + 'DD';
        ctx.fill();

        // Node stroke
        ctx.strokeStyle = isHov ? '#111111' : col + '88';
        ctx.lineWidth = isHov ? 2 : 0.5;
        ctx.stroke();

        // Label
        if (showLabels || isHov) {
          ctx.font = `${isHov ? 600 : 400} 10px Inter, sans-serif`;
          ctx.fillStyle = '#111111';
          ctx.textAlign = 'center';
          const label = nd.label.length > 30 ? nd.label.slice(0, 28) + '...' : nd.label;
          ctx.fillText(label, nd.x, nd.y + nd.r + 14);
        }
      }

      ctx.restore();

      // Tooltip for hovered node (screen-space)
      if (hovered) {
        const sx = hovered.x * zoom + panX;
        const sy = hovered.y * zoom + panY;
        const text = hovered.label;
        const typeBadge = `${hovered.kind}: ${hovered.type}`;

        ctx.font = '500 12px Inter, sans-serif';
        const tw = Math.max(ctx.measureText(text).width, ctx.measureText(typeBadge).width);
        const px = Math.min(sx + 16, w - tw - 32);
        const py = Math.max(sy - 20, 30);

        // Tooltip bg
        ctx.fillStyle = 'rgba(255, 255, 255, 0.92)';
        ctx.strokeStyle = 'rgba(17, 17, 17, 0.1)';
        ctx.lineWidth = 1;
        const bw = tw + 24;
        ctx.fillRect(px, py - 16, bw, 42);
        ctx.strokeRect(px, py - 16, bw, 42);

        // Shadow
        ctx.fillStyle = 'rgba(0, 0, 0, 0.04)';
        ctx.fillRect(px + 3, py - 13, bw, 42);

        // Text
        ctx.fillStyle = '#111111';
        ctx.font = '600 12px Inter, sans-serif';
        ctx.textAlign = 'left';
        ctx.fillText(text.length > 40 ? text.slice(0, 38) + '...' : text, px + 12, py + 2);
        ctx.fillStyle = '#999999';
        ctx.font = '400 10px Inter, sans-serif';
        ctx.fillText(typeBadge, px + 12, py + 18);
      }

      raf = requestAnimationFrame(draw);
    }

    raf = requestAnimationFrame(draw);

    // --- Interaction ---
    function screenToWorld(sx: number, sy: number): [number, number] {
      return [(sx - panX) / zoom, (sy - panY) / zoom];
    }

    function findNode(sx: number, sy: number): GraphNode | null {
      const [wx, wy] = screenToWorld(sx, sy);
      for (let i = nodes.length - 1; i >= 0; i--) {
        const nd = nodes[i];
        const dx = nd.x - wx;
        const dy = nd.y - wy;
        if (dx * dx + dy * dy < (nd.r + 4) * (nd.r + 4)) return nd;
      }
      return null;
    }

    function handleMouseDown(e: MouseEvent) {
      const rect = canvas.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;
      const nd = findNode(sx, sy);
      if (nd) {
        dragging = nd;
        const [wx, wy] = screenToWorld(sx, sy);
        dragOffsetX = nd.x - wx;
        dragOffsetY = nd.y - wy;
        nd.vx = 0;
        nd.vy = 0;
      } else {
        isPanning = true;
        panStartX = sx - panX;
        panStartY = sy - panY;
      }
    }

    function handleMouseMove(e: MouseEvent) {
      const rect = canvas.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;

      if (dragging) {
        const [wx, wy] = screenToWorld(sx, sy);
        dragging.x = wx + dragOffsetX;
        dragging.y = wy + dragOffsetY;
        dragging.vx = 0;
        dragging.vy = 0;
      } else if (isPanning) {
        panX = sx - panStartX;
        panY = sy - panStartY;
      } else {
        hovered = findNode(sx, sy);
        canvas.style.cursor = hovered ? 'grab' : 'default';
      }
    }

    function handleMouseUp() {
      dragging = null;
      isPanning = false;
    }

    function handleWheel(e: WheelEvent) {
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const sx = e.clientX - rect.left;
      const sy = e.clientY - rect.top;

      const oldZoom = zoom;
      const delta = e.deltaY > 0 ? 0.9 : 1.1;
      zoom = Math.max(0.3, Math.min(5, zoom * delta));

      // Zoom toward cursor
      panX = sx - (sx - panX) * (zoom / oldZoom);
      panY = sy - (sy - panY) * (zoom / oldZoom);
    }

    canvas.addEventListener('mousedown', handleMouseDown);
    canvas.addEventListener('mousemove', handleMouseMove);
    canvas.addEventListener('mouseup', handleMouseUp);
    canvas.addEventListener('mouseleave', handleMouseUp);
    canvas.addEventListener('wheel', handleWheel, { passive: false });

    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener('resize', resize);
      canvas.removeEventListener('mousedown', handleMouseDown);
      canvas.removeEventListener('mousemove', handleMouseMove);
      canvas.removeEventListener('mouseup', handleMouseUp);
      canvas.removeEventListener('mouseleave', handleMouseUp);
      canvas.removeEventListener('wheel', handleWheel);
    };
  });

  function togglePause() {
    running = !running;
  }
</script>

<div class="graph-container">
  <canvas bind:this={canvas} aria-hidden="true"></canvas>

  <div class="controls">
    <button class="ctrl-btn" onclick={togglePause}>
      {running ? 'Pause' : 'Play'}
    </button>
  </div>

  <div class="legend">
    <div class="legend-title">Graph</div>
    <div class="legend-stat">{nodeCount} nodes &middot; {edgeCount} edges</div>
    <div class="legend-section">Memories</div>
    {#each [['architecture', '#B8860B'], ['fact', '#2D6A4F'], ['pattern', '#2563EB'], ['workflow', '#1D4E89'], ['bug', '#CC0000']] as [type, color]}
      <div class="legend-item">
        <span class="legend-dot" style="background: {color}"></span>
        <span class="legend-label">{type}</span>
      </div>
    {/each}
    <div class="legend-section">Observations</div>
    {#each [['command_run', '#0E7490'], ['file_edit', '#C2410C'], ['file_read', '#2D6A4F'], ['file_write', '#B8860B'], ['conversation', '#6B3FA0'], ['search', '#2563EB']] as [type, color]}
      <div class="legend-item">
        <span class="legend-dot legend-dot--sm" style="background: {color}"></span>
        <span class="legend-label">{type}</span>
      </div>
    {/each}
  </div>
</div>

<style>
  .graph-container {
    position: relative;
    width: 100%;
    height: 100%;
  }

  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }

  .controls {
    position: absolute;
    bottom: 16px;
    right: 16px;
    z-index: 2;
  }

  .ctrl-btn {
    padding: 6px 14px;
    font-size: 10px;
    font-weight: 600;
    font-family: var(--font-ui);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--ink);
    cursor: pointer;
    transition: box-shadow 150ms;
  }
  .ctrl-btn:hover {
    box-shadow: 2px 2px 0 0 var(--border);
  }

  .legend {
    position: absolute;
    top: 12px;
    right: 12px;
    padding: 12px 16px;
    background: rgba(249, 249, 247, 0.92);
    border: 1px solid var(--border);
    z-index: 2;
    min-width: 140px;
  }

  .legend-title {
    font-family: var(--font-display);
    font-size: 13px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    margin-bottom: 4px;
    padding-bottom: 6px;
    border-bottom: 1px solid var(--border-light);
  }

  .legend-stat {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--ink-faint);
    margin-bottom: 10px;
  }

  .legend-section {
    font-family: var(--font-ui);
    font-size: 9px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--ink-muted);
    margin: 8px 0 4px;
  }

  .legend-item {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 2px;
  }

  .legend-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .legend-dot--sm {
    width: 7px;
    height: 7px;
  }

  .legend-label {
    font-size: 10px;
    font-family: var(--font-ui);
    color: var(--ink-secondary);
  }
</style>
