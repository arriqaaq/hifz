// SPDX-License-Identifier: Apache-2.0
// Single source of render colour: fetched once from /api/v1/render/tokens
// (memdiff::theme::tokens) so terminal and browser share one palette.
import { getRenderTokens } from './api';
import type { RenderTokens, Tone, ChangeOp } from './types';

const FALLBACK: RenderTokens = {
  plain: '#c9d1d9',
  added: '#7ee78a',
  revised: '#79c0ff',
  removed: '#ff7b72',
  linked: '#d2a8ff',
  conflict: '#f85149',
  muted: '#8b949e',
  cite: '#58a6ff',
};

let cache: RenderTokens | null = null;

export async function loadTokens(): Promise<RenderTokens> {
  if (cache) return cache;
  try {
    cache = await getRenderTokens();
  } catch {
    cache = FALLBACK;
  }
  return cache;
}

export function toneColor(tokens: RenderTokens | null, tone: Tone): string {
  return (tokens ?? FALLBACK)[tone] ?? FALLBACK.plain;
}

// The glyph cell takes the colour of the op's dominant tone.
export function opTone(op: ChangeOp): Tone {
  switch (op) {
    case 'created':
      return 'added';
    case 'superseded':
    case 'forgotten':
      return 'removed';
    case 'revised':
    case 'neighbour_revised':
      return 'revised';
    case 'linked':
      return 'linked';
    case 'conflict':
      return 'conflict';
    default:
      return 'plain';
  }
}
