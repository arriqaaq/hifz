<script lang="ts">
  import CytoscapeGraph, {
    type GraphInputNode,
    type GraphInputEdge,
  } from '$lib/components/graph/CytoscapeGraph.svelte';
  import type { AtlasGraph } from '$lib/api';

  let { graph }: { graph: AtlasGraph | null } = $props();

  // The /atlas/graph endpoint returns RecordId-typed `id`/`in`/`out`. The same
  // extractor runs on nodes and edges, so endpoints resolve to node ids
  // consistently regardless of the exact serialization shape.
  function extractId(v: unknown): string {
    if (typeof v === 'string') return v;
    if (v && typeof v === 'object') {
      const o = v as Record<string, unknown>;
      if (typeof o.key === 'string') return o.key;
      if (o.key && typeof o.key === 'object' && 'String' in (o.key as Record<string, unknown>))
        return (o.key as { String: string }).String;
      if (typeof o.id === 'string') return o.id;
    }
    return String(v);
  }

  let nodes = $derived<GraphInputNode[]>(
    (graph?.nodes ?? []).map((n) => {
      const o = n as Record<string, unknown>;
      const kind = (o.kind as string) ?? 'document';
      return {
        id: extractId(o.id),
        label: (o.label as string) ?? extractId(o.id),
        type: kind,
        kind: kind as GraphInputNode['kind'],
      };
    }),
  );
  let edges = $derived<GraphInputEdge[]>(
    (graph?.edges ?? []).map((e) => {
      const o = e as Record<string, unknown>;
      return { source: extractId(o.in), target: extractId(o.out) };
    }),
  );
</script>

<div class="map">
  <CytoscapeGraph {nodes} {edges} compact />
</div>

<style>
  .map {
    height: 360px;
    width: 100%;
  }
</style>
