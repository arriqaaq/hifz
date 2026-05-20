import type { StylesheetJson } from 'cytoscape';

// Colors per *content* type (memory category + obs_type). Phase 1+2 typed
// categories are mirrored from website/src/lib/ontology.ts. obs_type values
// stay as-is (they're free-form hook names).
export const TYPE_COLORS: Record<string, string> = {
  // typed memory categories (Phase 2 Category enum)
  observation: '#9ca3af',
  lesson: '#eab308',
  decision: '#9b59b6',
  bug: '#e74c3c',
  fix: '#22c55e',
  gotcha: '#f59e0b',
  convention: '#64748b',
  failure_pattern: '#f43f5e',
  plan: '#3b82f6',
  design: '#06b6d4',
  code_review: '#14b8a6',
  ship_report: '#10b981',
  context_slice: '#6366f1',
  note: '#737373',
  // observation types
  command_run: '#0E7490',
  file_edit: '#C2410C',
  file_read: '#15803D',
  file_write: '#B45309',
  conversation: '#6B3FA0',
  search: '#2563EB',
  commit_made: '#C2410C',
  // maktab corpus node kinds (concept map)
  document: '#2563EB',
  concept: '#9b59b6',
  code_symbol: '#15803D',
  external: '#94a3b8',
  file: '#B45309',
  other: '#6a6a63',
};

// Colors per *entity kind* — used for shape & border tint.
export const KIND_COLORS: Record<string, string> = {
  session: '#2563EB',
  run: '#15803D',
  observation: '#B45309',
  memory: '#6B3FA0',
  commit: '#C2410C',
  document: '#2563EB',
  concept: '#9b59b6',
  code_symbol: '#15803D',
  external: '#94a3b8',
  file: '#B45309',
};

export function colorFor(type: string): string {
  return TYPE_COLORS[type] ?? '#6a6a63';
}

export function kindColor(kind: string): string {
  return KIND_COLORS[kind] ?? '#6a6a63';
}

export const stylesheet: StylesheetJson = [
  {
    selector: 'node',
    style: {
      'background-color': 'data(color)',
      'border-color': '#1a1a1a',
      'border-opacity': 0.85,
      'border-width': 1.5,
      label: 'data(label)',
      'font-size': 10,
      'font-family': 'Inter, sans-serif',
      'font-weight': 500,
      color: '#1a1a1a',
      'text-valign': 'bottom',
      'text-margin-y': 6,
      'text-max-width': '140px',
      'text-wrap': 'ellipsis',
      'text-opacity': 0,
      width: 'data(size)',
      height: 'data(size)',
      'transition-property': 'opacity, background-color, border-color, border-width',
      'transition-duration': 200,
    },
  },
  // Per-kind shapes — each entity has a distinct silhouette.
  {
    selector: 'node[kind = "session"]',
    style: { shape: 'round-rectangle', 'border-width': 2 },
  },
  {
    selector: 'node[kind = "run"]',
    style: { shape: 'diamond', 'border-width': 2 },
  },
  {
    selector: 'node[kind = "observation"]',
    style: { shape: 'ellipse' },
  },
  {
    selector: 'node[kind = "memory"]',
    style: { shape: 'hexagon', 'border-width': 2 },
  },
  {
    selector: 'node[kind = "commit"]',
    style: { shape: 'tag', 'border-width': 2 },
  },
  // maktab corpus kinds (concept map)
  {
    selector: 'node[kind = "document"]',
    style: { shape: 'round-rectangle', 'border-width': 2 },
  },
  {
    selector: 'node[kind = "concept"]',
    style: { shape: 'ellipse' },
  },
  {
    selector: 'node[kind = "code_symbol"]',
    style: { shape: 'diamond' },
  },

  // Selection / interaction states — neon ring on selection
  {
    selector: 'node:selected',
    style: {
      'background-color': '#d9f400',
      'border-color': '#1a1a1a',
      'border-width': 3,
      'text-opacity': 1,
      'font-weight': 700,
      color: '#1a1a1a',
    },
  },
  {
    selector: 'node.locked',
    style: {
      'border-style': 'double',
      'border-width': 3,
    },
  },
  {
    selector: 'node.faded',
    style: { opacity: 0.12, 'text-opacity': 0 },
  },
  {
    selector: 'node.show-label',
    style: { 'text-opacity': 1 },
  },

  // Edges
  {
    selector: 'edge',
    style: {
      width: 1,
      'line-color': '#9a9a91',
      'line-opacity': 0.55,
      'curve-style': 'bezier',
      'target-arrow-shape': 'none',
      label: 'data(rel)',
      'font-size': 8,
      'font-family': 'JetBrains Mono, monospace',
      'font-weight': 500,
      color: '#6a6a63',
      'text-rotation': 'autorotate',
      'text-background-color': '#f2f1eb',
      'text-background-opacity': 0.9,
      'text-background-padding': '2px',
      'text-margin-y': -2,
      'text-opacity': 0,
      'transition-property': 'opacity, line-opacity, text-opacity',
      'transition-duration': 150,
    },
  },
  {
    selector: 'edge.show-label',
    style: { 'text-opacity': 1 },
  },
  {
    selector: 'edge.faded',
    style: { opacity: 0.06 },
  },

  // Typed relations — uppercase rel data: IN_SESSION, IN_RUN, SHARES_FILE, RECALLS, DISTILLED_FROM, PRODUCED_BY
  {
    selector: 'edge[rel = "IN_SESSION"]',
    style: {
      'line-color': '#2563EB',
      'line-style': 'solid',
      width: 1.5,
      'line-opacity': 0.6,
    },
  },
  {
    selector: 'edge[rel = "IN_RUN"]',
    style: {
      'line-color': '#15803D',
      'line-style': 'solid',
      width: 1.5,
      'line-opacity': 0.6,
    },
  },
  {
    selector: 'edge[rel = "SHARES_FILE"]',
    style: {
      'line-color': '#B45309',
      'line-style': 'solid',
      width: 1.5,
      'line-opacity': 0.6,
    },
  },
  {
    selector: 'edge[rel = "RECALLS"]',
    style: {
      'line-color': '#6B3FA0',
      'line-style': 'dashed',
      width: 1.5,
      'line-opacity': 0.6,
    },
  },
  {
    selector: 'edge[rel = "DISTILLED_FROM"]',
    style: {
      'line-color': '#6B3FA0',
      'line-style': 'dotted',
      width: 1.5,
      'line-opacity': 0.6,
    },
  },
  {
    selector: 'edge[rel = "PRODUCED_BY"]',
    style: {
      'line-color': '#C2410C',
      'line-style': 'solid',
      width: 1.5,
      'line-opacity': 0.6,
    },
  },

  // ---------------------------------------------------------------------
  // Phase 8.6: typed relation groups (Co-occurrence / Provenance /
  // Conceptual / Argumentative / Lifecycle / Code-domain). Per-group
  // color + style. `contradicts` overrides to red. Kept compatible with
  // the legacy IN_SESSION etc. selectors above which the export endpoint
  // still emits.
  // ---------------------------------------------------------------------

  // Co-occurrence — thin gray dashed
  { selector: 'edge[rel = "co_occurs_files"]',     style: { 'line-color': '#94a3b8', 'line-style': 'dashed', width: 1, 'line-opacity': 0.55 } },
  { selector: 'edge[rel = "co_occurs_keywords"]',  style: { 'line-color': '#94a3b8', 'line-style': 'dashed', width: 1, 'line-opacity': 0.55 } },
  { selector: 'edge[rel = "co_occurs_embedding"]', style: { 'line-color': '#94a3b8', 'line-style': 'dashed', width: 1, 'line-opacity': 0.55 } },
  { selector: 'edge[rel = "mentions"]',            style: { 'line-color': '#94a3b8', 'line-style': 'dashed', width: 1, 'line-opacity': 0.55 } },

  // Provenance — thin blue solid w/ arrow
  { selector: 'edge[rel = "generated_by"]',  style: { 'line-color': '#3b82f6', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#3b82f6' } },
  { selector: 'edge[rel = "informed_by"]',   style: { 'line-color': '#3b82f6', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#3b82f6' } },
  { selector: 'edge[rel = "derived_from"]',  style: { 'line-color': '#3b82f6', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#3b82f6' } },
  { selector: 'edge[rel = "attributed_to"]', style: { 'line-color': '#3b82f6', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#3b82f6' } },
  { selector: 'edge[rel = "part_of"]',       style: { 'line-color': '#3b82f6', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#3b82f6' } },
  { selector: 'edge[rel = "follows"]',       style: { 'line-color': '#3b82f6', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#3b82f6' } },

  // Conceptual — medium green solid
  { selector: 'edge[rel = "broader"]',  style: { 'line-color': '#22c55e', 'line-style': 'solid', width: 2, 'line-opacity': 0.7 } },
  { selector: 'edge[rel = "narrower"]', style: { 'line-color': '#22c55e', 'line-style': 'solid', width: 2, 'line-opacity': 0.7 } },
  { selector: 'edge[rel = "related"]',  style: { 'line-color': '#22c55e', 'line-style': 'solid', width: 2, 'line-opacity': 0.7 } },
  { selector: 'edge[rel = "same_as"]',  style: { 'line-color': '#22c55e', 'line-style': 'solid', width: 2, 'line-opacity': 0.7 } },

  // Argumentative — medium orange solid; `contradicts` distinct red
  { selector: 'edge[rel = "supports"]',     style: { 'line-color': '#f97316', 'line-style': 'solid', width: 2, 'line-opacity': 0.7 } },
  { selector: 'edge[rel = "contradicts"]',  style: { 'line-color': '#ef4444', 'line-style': 'solid', width: 2.5, 'line-opacity': 0.85 } },
  { selector: 'edge[rel = "elaborates"]',   style: { 'line-color': '#f97316', 'line-style': 'solid', width: 2, 'line-opacity': 0.7 } },
  { selector: 'edge[rel = "responds_to"]',  style: { 'line-color': '#f97316', 'line-style': 'solid', width: 2, 'line-opacity': 0.7 } },

  // Lifecycle — medium purple
  { selector: 'edge[rel = "supersedes"]', style: { 'line-color': '#a855f7', 'line-style': 'solid', width: 2, 'line-opacity': 0.75, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#a855f7' } },
  { selector: 'edge[rel = "closes"]',     style: { 'line-color': '#a855f7', 'line-style': 'solid', width: 2, 'line-opacity': 0.75, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#a855f7' } },

  // Code-domain — medium yellow
  { selector: 'edge[rel = "touches_file"]', style: { 'line-color': '#eab308', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6 } },
  { selector: 'edge[rel = "commits_for"]',  style: { 'line-color': '#eab308', 'line-style': 'solid', width: 2, 'line-opacity': 0.7, 'target-arrow-shape': 'triangle', 'target-arrow-color': '#eab308' } },
  { selector: 'edge[rel = "tests"]',        style: { 'line-color': '#eab308', 'line-style': 'solid', width: 1.5, 'line-opacity': 0.6 } },

  // Legacy classes (still emitted by causality.ts on the timeline mini-graph)
  {
    selector: 'edge.causal-file',
    style: {
      'line-color': '#15803D',
      'line-style': 'solid',
      width: 2,
      'line-opacity': 0.75,
    },
  },
  {
    selector: 'edge.causal-keyword',
    style: {
      'line-color': '#B45309',
      'line-style': 'dashed',
      width: 1.5,
      'line-opacity': 0.65,
    },
  },
  {
    selector: 'edge.causal-memory',
    style: {
      'line-color': '#6B3FA0',
      'line-style': 'dotted',
      width: 1.5,
      'line-opacity': 0.65,
    },
  },
];
