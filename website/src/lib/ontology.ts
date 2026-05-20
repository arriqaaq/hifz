/**
 * hifz typed ontology mirror.
 *
 * Source of truth: src/models.rs (Rust). This file MUST stay in sync.
 * scripts/sync-ontology.mjs regenerates this from the Rust source.
 * Until then, edit by hand and keep both sides aligned.
 *
 * See docs/ontology.md for the canonical reference.
 */

// --- Categories ---------------------------------------------------------

// === GENERATED CATEGORIES BEGIN — do not edit by hand ===
export const CATEGORIES = [
  'observation',
  'lesson',
  'decision',
  'bug',
  'fix',
  'gotcha',
  'convention',
  'failure_pattern',
  'plan',
  'design',
  'code_review',
  'ship_report',
  'context_slice',
  'note',
] as const;
// === GENERATED CATEGORIES END ===

export type Category = (typeof CATEGORIES)[number];

export const LONG_FORM_CATEGORIES: ReadonlySet<Category> = new Set([
  'plan',
  'design',
  'code_review',
  'ship_report',
  'context_slice',
]);

export function isLongForm(c: string): boolean {
  return LONG_FORM_CATEGORIES.has(c as Category);
}

/** Display label for a category (Title-cased, with `_` → space). */
export function categoryLabel(c: string): string {
  return c
    .split('_')
    .map((p) => p.charAt(0).toUpperCase() + p.slice(1))
    .join(' ');
}

/** Color token (mapped to CSS in graphStyles + memories list badge). */
export function categoryColor(c: string): string {
  switch (c as Category) {
    case 'plan':
      return 'badge-blue';
    case 'design':
      return 'badge-cyan';
    case 'decision':
      return 'badge-purple';
    case 'bug':
      return 'badge-red';
    case 'fix':
      return 'badge-green';
    case 'lesson':
      return 'badge-yellow';
    case 'gotcha':
      return 'badge-amber';
    case 'convention':
      return 'badge-slate';
    case 'failure_pattern':
      return 'badge-rose';
    case 'code_review':
      return 'badge-teal';
    case 'ship_report':
      return 'badge-emerald';
    case 'context_slice':
      return 'badge-indigo';
    case 'observation':
      return 'badge-gray';
    case 'note':
    default:
      return 'badge-neutral';
  }
}

// --- Edge relations ----------------------------------------------------

export const RELATION_GROUPS = {
  cooccurrence: ['co_occurs_files', 'co_occurs_keywords', 'co_occurs_embedding', 'mentions'],
  provenance: [
    'generated_by',
    'informed_by',
    'derived_from',
    'attributed_to',
    'part_of',
    'follows',
  ],
  conceptual: ['broader', 'narrower', 'related', 'same_as'],
  argumentative: ['supports', 'contradicts', 'elaborates', 'responds_to'],
  lifecycle: ['supersedes', 'closes'],
  codeDomain: ['touches_file', 'commits_for', 'tests'],
} as const;

export type RelationGroup = keyof typeof RELATION_GROUPS;

export const ALL_RELATIONS: readonly string[] = Object.values(RELATION_GROUPS).flat();

// === GENERATED RELATIONS BEGIN — do not edit by hand ===
export const ALL_RELATIONS_RAW: readonly string[] = [
  'co_occurs_files',
  'co_occurs_keywords',
  'co_occurs_embedding',
  'mentions',
  'generated_by',
  'informed_by',
  'derived_from',
  'attributed_to',
  'part_of',
  'follows',
  'broader',
  'narrower',
  'related',
  'same_as',
  'supports',
  'contradicts',
  'elaborates',
  'responds_to',
  'supersedes',
  'closes',
  'touches_file',
  'commits_for',
  'tests',
] as const;
// === GENERATED RELATIONS END ===

export function relationGroup(relation: string): RelationGroup | 'other' {
  for (const [group, list] of Object.entries(RELATION_GROUPS) as [RelationGroup, readonly string[]][]) {
    if (list.includes(relation)) return group;
  }
  return 'other';
}

/** CSS color token for an edge group, used in graph stylesheet + relation panel. */
export function relationGroupColor(group: RelationGroup | 'other'): string {
  switch (group) {
    case 'cooccurrence':
      return '#94a3b8'; // slate-400
    case 'provenance':
      return '#3b82f6'; // blue-500
    case 'conceptual':
      return '#22c55e'; // green-500
    case 'argumentative':
      return '#f97316'; // orange-500
    case 'lifecycle':
      return '#a855f7'; // purple-500
    case 'codeDomain':
      return '#eab308'; // yellow-500
    case 'other':
    default:
      return '#9ca3af'; // gray-400
  }
}

/** Human label for an edge group, used in detail-page section headers. */
export function relationGroupLabel(group: RelationGroup | 'other'): string {
  switch (group) {
    case 'cooccurrence':
      return 'Co-occurrence';
    case 'provenance':
      return 'Provenance';
    case 'conceptual':
      return 'Conceptual';
    case 'argumentative':
      return 'Argumentative';
    case 'lifecycle':
      return 'Lifecycle';
    case 'codeDomain':
      return 'Code-domain';
    case 'other':
    default:
      return 'Other';
  }
}

/** `contradicts` is the one relation that gets a distinct color (red). */
export function relationStyleOverride(relation: string): { color?: string; style?: 'solid' | 'dashed' } | null {
  if (relation === 'contradicts') {
    return { color: '#ef4444', style: 'solid' }; // red-500
  }
  if (relationGroup(relation) === 'cooccurrence') {
    return { style: 'dashed' };
  }
  return null;
}
