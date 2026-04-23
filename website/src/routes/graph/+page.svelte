<script lang="ts">
  import { onMount } from 'svelte';
  import { getExport } from '$lib/api';
  import MemoryGraph from '$lib/components/graph/MemoryGraph.svelte';
  import LoadingSpinner from '$lib/components/common/LoadingSpinner.svelte';

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

  let nodes = $state<GraphNode[]>([]);
  let edges = $state<GraphEdge[]>([]);
  let loading = $state(true);
  let error = $state('');

  onMount(async () => {
    try {
      const data = await getExport() as Record<string, unknown[]>;

      const memories = (data.memories ?? []) as Array<{
        id: string; title: string; category: string; keywords?: string[];
      }>;
      const observations = (data.observations ?? []) as Array<{
        id: string; title: string; obs_type: string; keywords?: string[];
      }>;

      const conceptMap = new Map<string, number[]>();
      let idx = 0;

      for (const m of memories) {
        const i = idx++;
        nodes.push({
          id: m.id, label: m.title, type: m.category, kind: 'memory',
          x: 0, y: 0, vx: 0, vy: 0, r: 14,
        });
        for (const c of m.keywords ?? []) {
          if (!conceptMap.has(c)) conceptMap.set(c, []);
          conceptMap.get(c)!.push(i);
        }
      }

      for (const o of observations) {
        if (o.title === 'unknown call' || o.obs_type === 'conversation') continue;
        const i = idx++;
        nodes.push({
          id: o.id, label: o.title, type: o.obs_type, kind: 'observation',
          x: 0, y: 0, vx: 0, vy: 0, r: 7,
        });
        for (const c of o.keywords ?? []) {
          if (!conceptMap.has(c)) conceptMap.set(c, []);
          conceptMap.get(c)!.push(i);
        }
      }

      // Build edges from shared concepts
      const edgeSet = new Set<string>();
      const degreeCount = new Array(nodes.length).fill(0);
      for (const indices of conceptMap.values()) {
        if (indices.length > 20) continue; // skip very common concepts
        for (let i = 0; i < indices.length; i++) {
          for (let j = i + 1; j < indices.length; j++) {
            const a = indices[i], b = indices[j];
            const key = a < b ? `${a}-${b}` : `${b}-${a}`;
            if (!edgeSet.has(key)) {
              edgeSet.add(key);
              edges.push({ source: a, target: b });
              degreeCount[a]++;
              degreeCount[b]++;
            }
          }
        }
      }

      // Scale node radius by degree
      for (let i = 0; i < nodes.length; i++) {
        const base = nodes[i].kind === 'memory' ? 12 : 6;
        nodes[i].r = Math.max(base, Math.min(22, base + degreeCount[i] * 2));
      }

    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load';
    } finally {
      loading = false;
    }
  });
</script>

<div class="graph-page">
  {#if loading}
    <LoadingSpinner />
  {:else if error}
    <div class="card" style="border-color: var(--accent);">
      <p style="color: var(--accent); margin: 0;">{error}</p>
    </div>
  {:else if nodes.length === 0}
    <p class="empty">No data to visualize</p>
  {:else}
    <MemoryGraph {nodes} {edges} />
  {/if}
</div>

<style>
  .graph-page {
    height: calc(100vh - 120px);
    position: relative;
  }
  .empty {
    text-align: center;
    color: var(--ink-faint);
    padding: 40px;
    font-family: var(--font-ui);
    font-size: 13px;
  }
</style>
