# Code indexing

hifz can index a code repository into the same graph that holds your memories. A `code_file → code_chunk` and `code_file → code_symbol` tree gets co-resident with `memory`, and a memory can reference a precise location:

```
Memory("BM25 analyzer is defined in src/db.rs:75-86")
  --references-->  CodeChunk { path: "src/db.rs", start_line: 75, end_line: 86 }
```

References are durable: when `src/db.rs` is reformatted or has lines inserted at the top, re-indexing remaps the edge to whichever new chunk overlaps the original lines.

This document is an operator's guide. For the architectural design, see the **Code dimension** section in [ARCHITECTURE.md](../ARCHITECTURE.md).

## Build

Code indexing is compiled into every hifz build — there is no feature flag.

```sh
cargo build                          # tree-sitter + 8 grammars always included
```

The build pulls in tree-sitter + 8 grammar crates (~5–10 MB binary growth); this is unconditional.

## Index a repo

CLI:

```sh
hifz index --project hifz --root /Users/me/workspace/hifz
```

REST:

```sh
curl -sS -X POST localhost:3111/api/v1/code/index \
  -H 'content-type: application/json' \
  -d '{"project":"hifz","root":"/abs/path/to/repo"}'
```

MCP (`hifz_code_index`):

```jsonc
{ "project": "hifz", "root": "/abs/path/to/repo",
  "follow_symlinks": false, "max_file_bytes": 2097152 }
```

Re-running `hifz index` on the same repo is **idempotent**: unchanged files (matching `mtime_ns + sha256`) are skipped. Expect `indexed=0, skipped_unchanged>0` on the second run.

### Supported languages (v1)

Rust · Python · JavaScript (incl. JSX, MJS, CJS) · TypeScript · TSX · Go · Java · C · C++

Anything else is invisible to the indexer. v2 backlog: Kotlin, Ruby, Swift, Scala, Markdown, TOML, YAML.

### What gets walked

- Honors `.gitignore` (root + parents), `.git/info/exclude`, global excludes — same engine as ripgrep.
- Skips files larger than `max_file_bytes` (default 2 MiB).
- Skips files whose first 512 bytes contain a NUL (binary heuristic).
- Skips hidden / dotfile entries by default.
- Only emits files whose extension maps to a supported language.

## Search

Hybrid (vector + BM25). Returns chunks with line ranges and snippets.

```sh
curl -sS -X POST localhost:3111/api/v1/code/search \
  -d '{"project":"hifz","query":"hybrid retrieval RRF","language":"rust","limit":5}'
```

Optional filters: `language`, `path` (substring match), `limit`, `group_by_file`.

`group_by_file=true` collapses results to one hit per file (the highest-scoring chunk wins).

## Cross-link a memory

### Auto-extracted (zero-effort)

When a memory's text contains a recognized pattern, `enrich::save_enriched` creates the edges automatically:

- `path/to/file.ext:NN[-MM]` → `references` edge to overlapping chunks.
- `…/blob/<sha>/path/file.rs#L42-L58` (GitHub permalink) → same.
- `module::name` or `Type::method` (qualified symbol) → `references_symbol` edge.

Bareword identifiers (`parse_chunk` standing alone) are **not** auto-linked — only qualified forms. This avoids false-positive link spam.

### Explicit

When the agent knows the exact location, use the explicit tools:

```sh
curl -sS -X POST localhost:3111/api/v1/code/link \
  -d '{"memory_id":"memory:abc","project":"hifz",
       "file":"src/search.rs","start_line":701,"end_line":730}'

curl -sS -X POST localhost:3111/api/v1/code/link/symbol \
  -d '{"memory_id":"memory:abc","project":"hifz",
       "name":"search_memory_chunks"}'
```

MCP equivalents: `hifz_link_code`, `hifz_link_symbol`.

## Inspect the graph

Existing memory tools work transparently across the new edge types:

```sh
# Outbound links — includes `references` and `references_symbol`
curl -sS localhost:3111/api/v1/memories/<id>/links

# Hybrid memory search — graph expansion now surfaces linked code rows
curl -sS -X POST localhost:3111/api/v1/search \
  -d '{"query":"BM25 analyzer","project":"hifz"}'
```

## Garbage collection

When files are deleted from disk, indexed rows linger until reconciled. Run `hifz_code_gc` (or the CLI subcommand) to clean up:

```sh
hifz code-gc --project hifz --root /abs/path/to/repo
hifz code-gc --project hifz --root /abs/path/to/repo --force-decay
```

REST:

```sh
curl -sS -X POST localhost:3111/api/v1/code/gc \
  -d '{"project":"hifz","root":"/abs/path/to/repo","dry_run":false}'
```

The GC pass:

1. Walks the filesystem, collects current paths.
2. Diffs against `code_file` rows for the project.
3. For each missing file: drops inbound `references` / `references_symbol` edges (audit-tagged with `metadata.dropped_reason='file_deleted'`), deletes owned chunks/symbols, sets `code_file.deleted_at` (30-day tombstone for audit).
4. *(With `force_decay=true`)* — chunks with no inbound refs and stale commit/age multiplicatively lose strength; below 0.1 they're deleted.

Tombstones are swept by `forget::run_forget` after 30 days.

## Live re-indexing

```sh
HIFZ_CODE_WATCH=1 \
HIFZ_CODE_WATCH_ROOTS=hifz=/path/to/hifz,docs=/path/to/docs \
hifz serve
```

Each `(project, root)` pair gets a debounced `notify` watcher (500 ms). Filesystem events trigger an idempotent re-index of the touched file.

## End-to-end smoke test

```sh
cargo run -- serve --memory --port 3111 &
PORT=3111

# 1. Index
curl -sS -X POST localhost:$PORT/api/v1/code/index \
  -d '{"project":"hifz","root":"/abs/path/to/hifz"}'

# 2. Search
curl -sS -X POST localhost:$PORT/api/v1/code/search \
  -d '{"project":"hifz","query":"hybrid retrieval","language":"rust"}'

# 3. Memory with auto-extracted reference
MID=$(curl -sS -X POST localhost:$PORT/api/v1/memories \
  -d '{"project":"hifz","title":"BM25","content":"see src/db.rs:75-86"}' \
  | jq -r .id)

# 4. Confirm the edge
curl -sS localhost:$PORT/api/v1/memories/$MID/links \
  | jq '.links[] | select(.relation=="references")'
```
