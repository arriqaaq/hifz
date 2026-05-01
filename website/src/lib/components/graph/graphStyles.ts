import type { StylesheetJson } from 'cytoscape';

export const TYPE_COLORS: Record<string, string> = {
  // memory categories
  architecture: '#B8860B',
  pattern: '#2563EB',
  fact: '#2D6A4F',
  workflow: '#1D4E89',
  bug: '#CC0000',
  preference: '#6B3FA0',
  // observation types
  command_run: '#0E7490',
  file_edit: '#C2410C',
  file_read: '#2D6A4F',
  file_write: '#B8860B',
  conversation: '#6B3FA0',
  search: '#2563EB',
  commit_made: '#1D4E89',
  other: '#999999',
};

export function colorFor(type: string): string {
  return TYPE_COLORS[type] ?? '#666666';
}

export const stylesheet: StylesheetJson = [
  {
    selector: 'node',
    style: {
      'background-color': 'data(color)',
      'border-color': 'data(color)',
      'border-opacity': 0.6,
      'border-width': 1,
      label: 'data(label)',
      'font-size': 10,
      'font-family': 'Inter, sans-serif',
      color: '#111111',
      'text-valign': 'bottom',
      'text-margin-y': 6,
      'text-max-width': '120px',
      'text-wrap': 'ellipsis',
      'text-opacity': 0,
      width: 'data(size)',
      height: 'data(size)',
    },
  },
  {
    selector: 'node[kind = "memory"]',
    style: {
      shape: 'round-rectangle',
      'border-width': 2,
    },
  },
  {
    selector: 'node[kind = "observation"]',
    style: {
      shape: 'ellipse',
    },
  },
  {
    selector: 'node:selected',
    style: {
      'border-color': '#111111',
      'border-width': 2,
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
    style: {
      opacity: 0.15,
      'text-opacity': 0,
    },
  },
  {
    selector: 'node.show-label',
    style: {
      'text-opacity': 1,
    },
  },
  {
    selector: 'edge',
    style: {
      width: 1,
      'line-color': '#B4B4AA',
      'line-opacity': 0.5,
      'curve-style': 'bezier',
      'target-arrow-shape': 'none',
    },
  },
  {
    selector: 'edge.faded',
    style: {
      opacity: 0.05,
    },
  },
  {
    selector: 'edge.causal-file',
    style: {
      'line-color': '#2D6A4F',
      'line-style': 'solid',
      width: 2,
      'line-opacity': 0.8,
    },
  },
  {
    selector: 'edge.causal-keyword',
    style: {
      'line-color': '#B8860B',
      'line-style': 'dashed',
      width: 1.5,
      'line-opacity': 0.7,
    },
  },
  {
    selector: 'edge.causal-memory',
    style: {
      'line-color': '#6B3FA0',
      'line-style': 'dotted',
      width: 1.5,
      'line-opacity': 0.7,
    },
  },
];
