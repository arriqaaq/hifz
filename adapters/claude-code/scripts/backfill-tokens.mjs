#!/usr/bin/env node
// One-time backfill of Claude Code token data into hifz.
//
//   node adapters/claude-code/scripts/backfill-tokens.mjs            (tracked sessions only)
//   node adapters/claude-code/scripts/backfill-tokens.mjs --include-untracked
//   node adapters/claude-code/scripts/backfill-tokens.mjs --project hifz
//
// Walks ~/.claude/projects/<dir>/<sessionId>.jsonl. By default only ingests
// transcripts whose sessionId is already known to hifz (queried once via
// GET /api/v1/agent/sessions). Idempotent — re-running is a no-op for
// already-imported records.

import { createReadStream } from "node:fs";
import { readdir, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";

import { parseTranscript } from "./parse-transcript.mjs";

const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";

const args = parseArgs(process.argv.slice(2));

async function main() {
  const claudeDir = join(homedir(), ".claude", "projects");
  let projectDirs;
  try {
    projectDirs = await readdir(claudeDir);
  } catch {
    console.error(`No Claude Code projects directory at ${claudeDir}.`);
    process.exit(0);
  }

  let trackedSet = null;
  if (!args.includeUntracked) {
    trackedSet = await fetchTrackedSessions();
    if (trackedSet === null) {
      console.error(
        "Couldn't reach hifz server — is `cargo run -- serve` running on " +
          REST_URL +
          "? Pass --include-untracked to skip the session filter.",
      );
      process.exit(1);
    }
    console.error(
      `Discovered ${trackedSet.size} hifz-tracked sessions; will ingest only those.`,
    );
  } else {
    console.error("--include-untracked: ingesting every transcript found.");
  }

  let filesScanned = 0;
  let filesIngested = 0;
  let recordsPosted = 0;
  let recordsInserted = 0;

  for (const proj of projectDirs) {
    const dir = join(claudeDir, proj);
    try {
      const s = await stat(dir);
      if (!s.isDirectory()) continue;
    } catch {
      continue;
    }
    let entries;
    try {
      entries = await collectTranscriptPaths(dir);
    } catch {
      continue;
    }

    for (const { path: filePath, sessionId } of entries) {
      filesScanned += 1;
      if (trackedSet && !trackedSet.has(sessionId)) continue;

      // If --project was passed, filter by matching project string. We don't
      // know the project from the directory name alone; ask hifz instead.
      let parsed;
      try {
        parsed = await parseTranscript(filePath, { sessionId });
      } catch (e) {
        console.error(`  skip ${sessionId}: parse error (${e.message ?? e})`);
        continue;
      }
      const { records } = parsed;
      if (records.length === 0) continue;

      // Override the project field if we have the hifz session's project.
      const project = trackedProject(trackedSet, sessionId, proj);
      for (const r of records) {
        if (project) r.project = project;
      }
      if (args.project && project && project !== args.project) {
        continue;
      }

      filesIngested += 1;
      recordsPosted += records.length;

      const res = await postBatch(records);
      if (res?.inserted != null) recordsInserted += res.inserted;
      process.stderr.write(
        `\r  scanned ${filesScanned} · ingested ${filesIngested} files · ${recordsInserted} new records`,
      );
    }
  }

  process.stderr.write("\n");
  console.log(
    JSON.stringify(
      {
        files_scanned: filesScanned,
        files_ingested: filesIngested,
        records_posted: recordsPosted,
        records_inserted: recordsInserted,
      },
      null,
      2,
    ),
  );
}

// Returns `[{path, sessionId}]` for every transcript under `projectDir`.
// Top-level transcripts live at `projectDir/<sessionId>.jsonl`; subagent
// transcripts spawned by Task calls live at
// `projectDir/<sessionId>/subagents/*.jsonl` and carry the parent session
// id on every row. We use the parent session id for the tracked-session
// filter — peeking the first JSONL row when the parent isn't obvious
// from the directory structure.
async function collectTranscriptPaths(projectDir) {
  const out = [];
  let topLevel;
  try {
    topLevel = await readdir(projectDir);
  } catch {
    return out;
  }
  for (const name of topLevel) {
    const fullPath = join(projectDir, name);
    if (name.endsWith(".jsonl")) {
      out.push({
        path: fullPath,
        sessionId: name.replace(/\.jsonl$/, ""),
      });
      continue;
    }
    // Per-session subdir: walk for `subagents/*.jsonl`.
    const subagentsRoot = join(fullPath, "subagents");
    let subagentFiles;
    try {
      subagentFiles = await readdir(subagentsRoot);
    } catch {
      continue;
    }
    for (const sub of subagentFiles) {
      if (!sub.endsWith(".jsonl")) continue;
      const subPath = join(subagentsRoot, sub);
      // Prefer the parent session id from the directory name; peek the
      // first row only as a fallback if the directory name isn't a UUID.
      const parentSession = isLikelyUuid(name)
        ? name
        : ((await peekSessionId(subPath)) ?? sub.replace(/\.jsonl$/, ""));
      out.push({ path: subPath, sessionId: parentSession });
    }
  }
  return out;
}

function isLikelyUuid(s) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s);
}

async function peekSessionId(path) {
  const stream = createReadStream(path, { encoding: "utf-8" });
  const rl = createInterface({ input: stream, crlfDelay: Infinity });
  try {
    for await (const line of rl) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const entry = JSON.parse(trimmed);
        if (entry?.sessionId) return entry.sessionId;
      } catch {
        // skip malformed line, keep scanning
      }
      // First valid line had no sessionId — give up and let caller fall
      // back to filename. (Avoids reading the whole file just for this.)
      return null;
    }
  } finally {
    rl.close();
    stream.destroy();
  }
  return null;
}

async function fetchTrackedSessions() {
  try {
    const res = await fetch(`${REST_URL}/api/v1/agent/sessions?limit=10000`, {
      signal: AbortSignal.timeout(10e3),
    });
    if (!res.ok) return null;
    const body = await res.json();
    const map = new Map();
    for (const s of body.sessions ?? []) {
      const id = extractSessionId(s);
      if (id) map.set(id, s.project ?? null);
    }
    map.lookupProject = (sid) => map.get(sid) ?? null;
    return map;
  } catch {
    return null;
  }
}

function trackedProject(map, sessionId, fallbackDirName) {
  if (!map) return fallbackDirName ?? null;
  return map.get(sessionId) ?? fallbackDirName ?? null;
}

function extractSessionId(s) {
  // Session ids come back from hifz as RecordIds — accommodate the various
  // serialised forms we've seen in the codebase.
  const id = s?.id;
  if (typeof id === "string") return id.replace(/^session:/, "");
  if (id && typeof id === "object") {
    if (typeof id.id === "string") return id.id;
    const k = id.key;
    if (typeof k === "string") return k;
    if (k && typeof k === "object" && typeof k.String === "string") return k.String;
  }
  return null;
}

async function postBatch(records) {
  try {
    const res = await fetch(`${REST_URL}/api/v1/agent/usage/batch`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ records }),
      signal: AbortSignal.timeout(30e3),
    });
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

function parseArgs(argv) {
  const out = { includeUntracked: false, project: null };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--include-untracked") out.includeUntracked = true;
    else if (a === "--project") out.project = argv[++i];
  }
  return out;
}

await main();
