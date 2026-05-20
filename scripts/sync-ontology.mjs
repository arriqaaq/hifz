#!/usr/bin/env node
// sync-ontology.mjs
//
// Generate the TypeScript ontology mirrors from crates/kernel/src/models.rs.
//
// Reads:  crates/kernel/src/models.rs (parses the `Category` and `EdgeRelation` enums)
// Writes:
//   - website/src/lib/ontology.ts
//   - adapters/pi-extension/src/ontology.ts
//
// Run via `make sync-ontology` (target added in Makefile). CI invokes the
// same target with `--check` to fail when the generated files would change
// (i.e. someone added a Rust enum variant without regenerating).
//
// The parser is deliberately small: it expects the canonical `pub enum Foo
// { Bar, Baz, ... }` form with `#[serde(rename_all = "snake_case")]`. If
// the form drifts, this script will fail loudly rather than silently
// produce a stale file.

import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MODELS = resolve(ROOT, "crates/kernel/src/models.rs");

const args = process.argv.slice(2);
const CHECK = args.includes("--check");

const src = readFileSync(MODELS, "utf8");

// Strip block + line comments; the parser doesn't need them.
const stripped = src
  .replace(/\/\*[\s\S]*?\*\//g, "")
  .replace(/\/\/[^\n]*/g, "");

function extractEnum(name) {
  const re = new RegExp(`pub enum ${name}\\s*\\{([\\s\\S]*?)\\}`, "m");
  const m = stripped.match(re);
  if (!m) throw new Error(`enum ${name} not found in ${MODELS}`);
  const body = m[1];
  // Variants are CamelCase identifiers possibly preceded by attributes.
  // Skip variants annotated with #[serde(other)] — they map to "Other"
  // (catch-all) and aren't part of the wire vocabulary.
  const variants = [];
  for (const line of body.split(/\n/)) {
    const trimmed = line.trim().replace(/,$/, "");
    if (!trimmed) continue;
    if (trimmed.startsWith("#")) continue;
    if (/^[A-Z][A-Za-z0-9]*$/.test(trimmed)) {
      variants.push(trimmed);
    }
  }
  return variants;
}

function camelToSnake(s) {
  return s
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2")
    .toLowerCase();
}

const categoryVariants = extractEnum("Category");
const relationVariants = extractEnum("EdgeRelation");

const categories = categoryVariants
  .filter((v) => v !== "Other")
  .map(camelToSnake);
const relations = relationVariants
  .filter((v) => v !== "Other")
  .map(camelToSnake);

// ---- generate website/src/lib/ontology.ts shape -----------------------
// Only regenerate the parts that derive from the enums; the rest of the
// file (color maps, label functions) is hand-maintained because it
// encodes UI choices the script can't infer.
//
// Strategy: between BEGIN/END markers, replace generated content. If the
// markers aren't present in the file, fail loudly so the human is aware.

function regenerateBetweenMarkers(filePath, markerName, generated) {
  if (!existsSync(filePath)) {
    throw new Error(`target file missing: ${filePath}`);
  }
  const before = readFileSync(filePath, "utf8");
  const beginMarker = `// === GENERATED ${markerName} BEGIN — do not edit by hand ===`;
  const endMarker = `// === GENERATED ${markerName} END ===`;
  const beginIdx = before.indexOf(beginMarker);
  const endIdx = before.indexOf(endMarker);
  if (beginIdx === -1 || endIdx === -1) {
    throw new Error(
      `${filePath}: missing generated-block markers for ${markerName}. ` +
        `Add lines:\n  ${beginMarker}\n  ${endMarker}\nbetween them put the existing typed content; sync-ontology will replace.`,
    );
  }
  const after =
    before.slice(0, beginIdx + beginMarker.length) +
    "\n" +
    generated +
    "\n" +
    before.slice(endIdx);
  return { before, after };
}

const websiteCategoriesBlock = `export const CATEGORIES = [
${categories.map((c) => `  '${c}',`).join("\n")}
] as const;`;

const websiteRelationsBlock = `export const ALL_RELATIONS_RAW: readonly string[] = [
${relations.map((r) => `  '${r}',`).join("\n")}
] as const;`;

const piCategoriesBlock = `export const CATEGORIES = [
${categories.map((c) => `  "${c}",`).join("\n")}
] as const;`;

const targets = [
  {
    file: resolve(ROOT, "website/src/lib/ontology.ts"),
    blocks: [
      { marker: "CATEGORIES", content: websiteCategoriesBlock },
      { marker: "RELATIONS", content: websiteRelationsBlock },
    ],
  },
  {
    file: resolve(ROOT, "adapters/pi-extension/src/ontology.ts"),
    blocks: [{ marker: "CATEGORIES", content: piCategoriesBlock }],
  },
];

let changed = false;
for (const t of targets) {
  let current = readFileSync(t.file, "utf8");
  for (const b of t.blocks) {
    const { after } = regenerateBetweenMarkers(t.file, b.marker, b.content);
    if (after !== current) {
      changed = true;
      if (CHECK) {
        console.error(
          `[sync-ontology] DRIFT in ${t.file} block "${b.marker}". ` +
            `Run \`make sync-ontology\` to regenerate.`,
        );
      } else {
        writeFileSync(t.file, after);
        console.log(`[sync-ontology] updated ${t.file} block "${b.marker}"`);
        current = after;
      }
    }
  }
}

if (CHECK && changed) process.exit(1);
if (!changed) console.log("[sync-ontology] up to date");
