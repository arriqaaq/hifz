import type { StylesheetJson } from 'cytoscape';

// Colors per *content* type (memory category, obs_type) — used for fill.
export const TYPE_COLORS: Record<string, string> = {
  architecture: '#B45309',
  pattern: '#2563EB',
  fact: '#15803D',
  workflow: '#1D4E89',
  bug: '#B91C1C',
  preference: '#6B3FA0',
  command_run: '#0E7490',
  file_edit: '#C2410C',
  file_read: '#15803D',
  file_write: '#B45309',
  conversation: '#6B3FA0',
  search: '#2563EB',
  commit_made: '#C2410C',
  other: '#6B6B6B',
};

// Colors per *entity kind* — used for shape & border tint.
export const KIND_COLORS: Record<string, string> = {
  session: '#2563EB',
  run: '#15803D',
  observation: '#B45309',
  memory: '#6B3FA0',
  commit: '#C2410C',
};

export function colorFor(type: string): string {
  return TYPE_COLORS[type] ?? '#6B6B6B';
}

export function kindColor(kind: string): string {
  return KIND_COLORS[kind] ?? '#6B6B6B';
}

export const stylesheet: StylesheetJson = [
  {
    selector: 'node',
    style: {
      'background-color': 'data(color)',
      'border-color': 'data(color)',
      'border-opacity': 0.7,
      'border-width': 1,
      label: 'data(label)',
      'font-size': 10,
      'font-family': 'Inter, sans-serif',
      color: '#1A1A1A',
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
  // Per-kind shapes — Resolve.ai pattern: each entity has a distinct silhouette.
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

  // Selection / interaction states
  {
    selector: 'node:selected',
    style: {
      'border-color': '#1A1A1A',
      'border-width': 3,
      'text-opacity': 1,
      'font-weight': 600,
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
      'line-color': '#C4C4BE',
      'line-opacity': 0.55,
      'curve-style': 'bezier',
      'target-arrow-shape': 'none',
      label: 'data(rel)',
      'font-size': 8,
      'font-family': 'Inter, sans-serif',
      'font-weight': 600,
      color: '#9B9B9B',
      'text-rotation': 'autorotate',
      'text-background-color': '#FAFAF9',
      'text-background-opacity': 0.85,
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
