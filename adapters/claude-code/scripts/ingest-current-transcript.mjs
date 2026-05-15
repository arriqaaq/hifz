#!/usr/bin/env node
// Live ingestion: parse one transcript JSONL and POST every record.
//
// Called from the Stop and SessionEnd hooks. Re-sending the whole file is
// safe — hifz dedupes on (agent, external_id) UNIQUE, so already-stored
// rows are no-ops. Simpler than tracking byte offsets, and JSONL parsing is
// fast (~50ms for a 200-turn session).
//
// Invocation:
//   echo '{"session_id":"...","transcript_path":"...","cwd":"..."}' \
//     | node ingest-current-transcript.mjs

import { parseTranscript } from "./parse-transcript.mjs";

const REST_URL = process.env["HIFZ_URL"] || "http://localhost:3111";
const HEADERS = { "Content-Type": "application/json" };

const RETRY_DELAYS_MS = [200, 600, 1800];
const REQUEST_TIMEOUT_MS = 3000;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export async function ingestTranscript(payload) {
  if (!payload?.transcript_path) return { posted: 0 };
  const sessionId = payload.session_id || undefined;
  const cwd = payload.cwd || undefined;

  let parsed;
  try {
    parsed = await parseTranscript(payload.transcript_path, {
      sessionId,
      cwd,
      project: payload.project || cwd,
    });
  } catch {
    return { posted: 0 };
  }
  const { records } = parsed;
  if (records.length === 0) return { posted: 0 };

  return await postWithRetry(`${REST_URL}/api/v1/agent/usage/batch`, {
    records,
  });
}

async function postWithRetry(url, body) {
  const payload = JSON.stringify(body);
  for (let attempt = 0; attempt <= RETRY_DELAYS_MS.length; attempt++) {
    try {
      const ctrl = new AbortController();
      const t = setTimeout(() => ctrl.abort(), REQUEST_TIMEOUT_MS);
      let res;
      try {
        res = await fetch(url, {
          method: "POST",
          headers: HEADERS,
          body: payload,
          signal: ctrl.signal,
        });
      } finally {
        clearTimeout(t);
      }
      if (res.ok) return await res.json();
      if (res.status >= 500 && attempt < RETRY_DELAYS_MS.length) {
        await sleep(RETRY_DELAYS_MS[attempt]);
        continue;
      }
      return { posted: 0, status: res.status };
    } catch {
      if (attempt < RETRY_DELAYS_MS.length) {
        await sleep(RETRY_DELAYS_MS[attempt]);
        continue;
      }
      return { posted: 0 };
    }
  }
  return { posted: 0 };
}

// Stdin entry point when run as a script.
if (
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith("/ingest-current-transcript.mjs")
) {
  let input = "";
  for await (const chunk of process.stdin) input += chunk;
  let payload;
  try {
    payload = JSON.parse(input);
  } catch {
    process.exit(0);
  }
  await ingestTranscript(payload);
}
