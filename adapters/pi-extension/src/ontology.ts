/**
 * hifz typed ontology mirror — Pi extension copy.
 *
 * Source of truth: src/models.rs (Rust) in the hifz repo. The website
 * mirror at website/src/lib/ontology.ts is the canonical TypeScript
 * version; this file should match it.
 *
 * Phase 9.3's `scripts/sync-ontology.mjs` will eventually generate both
 * files from the Rust source.
 */

// === GENERATED CATEGORIES BEGIN — do not edit by hand ===
export const CATEGORIES = [
  "observation",
  "lesson",
  "decision",
  "bug",
  "fix",
  "gotcha",
  "convention",
  "failure_pattern",
  "plan",
  "design",
  "code_review",
  "ship_report",
  "context_slice",
  "note",
] as const;
// === GENERATED CATEGORIES END ===

export type Category = (typeof CATEGORIES)[number];

export const LONG_FORM_CATEGORIES: ReadonlySet<Category> = new Set([
  "plan",
  "design",
  "code_review",
  "ship_report",
  "context_slice",
]);

export function isLongForm(c: string): boolean {
  return LONG_FORM_CATEGORIES.has(c as Category);
}
