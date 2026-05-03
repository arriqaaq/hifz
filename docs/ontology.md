# hifz ontology

This document is the source of truth for hifz's typed memory categories,
relation vocabulary, and the deterministic-vs-LLM mode matrix. The
website (`website/src/lib/ontology.ts`) and adapters
(`adapters/pi-extension/src/ontology.ts`) mirror this; if you add a
variant in [src/models.rs](../src/models.rs), update those mirrors too.

---

## Categories

Memories are typed via `Memory.category`, a string that maps to the
`Category` enum in [src/models.rs](../src/models.rs).

| Category | Long-form? | Typical use | Default link relations |
|---|---|---|---|
| `observation` | no | Ephemeral hook record promoted to memory | `derived_from` (from observation row) |
| `lesson` | no | Generalized learning across one or more sessions | `derived_from` (from session_summary) |
| `decision` | no | Architectural / scoping choice with stated reasoning | `supports` / `contradicts` (LLM) |
| `bug` | no | An open problem; closed by a `Fix` | targeted by `closes` from a Fix |
| `fix` | no | A fix for a `Bug`; should carry `closes` to it | `closes` |
| `gotcha` | no | A non-obvious behavior that surprised the agent | `mentions` to entities |
| `convention` | no | A project rule or pattern | `mentions` |
| `failure_pattern` | no | A recurring failure surfaced during review | `derived_from` to source memories |
| `plan` | yes | Long-form plan document | `supersedes` to prior plan |
| `design` | yes | Long-form design document | `elaborates` to a plan (LLM) |
| `code_review` | yes | Long-form code review report | `commits_for` (incoming) |
| `ship_report` | yes | Long-form ship/release verdict | `commits_for` (incoming) |
| `context_slice` | yes | Long-form project-context note | `mentions` to file entities |
| `note` | no | Catch-all for ad-hoc memories. Default. | (none) |

**Long-form behavior**: when `category.is_long_form()` is true, hifz expects
the body in `content_long` and a 1-2 sentence summary in `content`. The
`memory_chunk` table stores token-windowed chunks of `content_long` for
retrieval. The deterministic co-occurrence link pass is **skipped** for
long-form rows (they own intentional links via `supersedes` / `closes`).

Unknown category strings parse as `note` (lossy by design — agents
shouldn't crash on a typo).

---

## Relations

Edges are typed via the `EdgeRelation` enum in
[src/models.rs](../src/models.rs). Relations are grouped by **what
generated the edge**; the grouping makes the deterministic-vs-LLM
boundary visible in the type itself.

### Co-occurrence (deterministic, signal-typed)

| Relation | Endpoints | When | `via` |
|---|---|---|---|
| `co_occurs_files` | Memory → Memory | Two memories share file paths (`overlap_score` ≥ floor) | `file` |
| `co_occurs_keywords` | Memory → Memory | Two memories share caller-supplied keywords | `keyword` |
| `co_occurs_embedding` | Memory → Memory | Cosine distance < 0.25 | `embedding` |
| `mentions` | Memory → Entity \| Memory | Entity-shared link | `entity` |

These replace the old overloaded `similar_to`. The relation name now
records *which signal* produced the edge, not a fake semantic claim.

### Provenance (PROV-O grounded, deterministic, system-set)

| Relation | Endpoints | When |
|---|---|---|
| `generated_by` | Memory \| Observation → Run | Memory created or commit observation produced during a run |
| `informed_by` | Memory → Run | Memory was used as input to a run (recalled via search) |
| `derived_from` | Memory → Memory \| Observation | Memory derived from a recalled memory or source observation |
| `attributed_to` | Memory → Agent | Memory authored by a particular agent / model |
| `part_of` | Run → Session, Memory → Memory, Observation → Run, MemoryChunk → Memory | Structural containment |
| `follows` | Run → Run | Temporal sequence within a session |

### Conceptual (SKOS grounded, LLM-set or strong heuristic)

| Relation | Endpoints | When |
|---|---|---|
| `broader` | Memory → Memory | A is more general than B (LLM judgment) |
| `narrower` | Memory → Memory | A is more specific than B |
| `related` | Memory → Memory | Symmetric conceptual link the LLM can't decompose into broader/narrower |
| `same_as` | Memory → Memory | SKOS exactMatch — explicit dedup pointer. Does NOT merge identities like `owl:sameAs` would. |

### Argumentative (IBIS grounded, LLM-set)

| Relation | Endpoints | When |
|---|---|---|
| `supports` | Memory → Memory | A supports B's claim |
| `contradicts` | Memory → Memory | A conflicts with B (also surfaced as a `forget.rs` is_latest flip when content Jaccard ≥ 0.9) |
| `elaborates` | Memory → Memory | A adds detail to B |
| `responds_to` | Memory → Memory | A is a response to a question/issue raised in B |

### Lifecycle (deterministic when explicit, LLM otherwise)

| Relation | Endpoints | When |
|---|---|---|
| `supersedes` | Memory → Memory | A replaces B. Old row's `is_latest` is set to false. |
| `closes` | Memory → Memory | A closes/resolves B (e.g., a fix memory closes a bug memory). |

### Code-domain (deterministic, write-time)

| Relation | Endpoints | When |
|---|---|---|
| `touches_file` | Observation → Memory | Observation's `files` overlap with the memory's `files`. Score = `1.0 / matched_count`, capped at 20 memories per file. |
| `commits_for` | Observation → Memory | A `commit_made` observation's message BM25-matches an open Bug/Plan/Decision. Top-3 hits, score = `bm25 / 4` clamped. |
| `tests` | Memory → Memory | A memory describes tests for code another memory describes. |

### Catch-all

| Relation | Notes |
|---|---|
| `other` | Forward-compat: unknown relation strings deserialize here, are short-circuited as permitted by the type-pair validator. Deterministic linker never produces this. |

### Edge metadata

Every edge also carries:

- `via: string` — channel name (`embedding`, `keyword`, `file`, `entity`,
  `system`, `cluster`, `llm`).
- `score: float` — confidence/strength, ~`[0, 1]`. Higher is stronger.
- `reason: option<string>` — one-line audit note. For deterministic edges,
  the channel + score (`"keyword overlap 2 shared (score 1.00): jwt, auth"`).
  For LLM-set edges, the model's one-sentence rationale.
- `metadata: option<object>` — free-form JSON for adapter use.
- `created_at: string` — RFC3339.

### Type-pair constraints

Per-relation `(from_kind, relation, to_kind)` triples are validated at
write time in [src/link.rs](../src/link.rs)::`is_allowed_relation`.
Violations are logged at WARN and skipped (Phase 10 may flip to
hard-error). Unknown relation `Other` and unknown record kind `Other`
short-circuit to permitted.

---

## Mode matrix

Hifz operates in one of two modes per insert. Both modes are valid;
they differ in what the runtime can derive without an external service.

| Capability | Deterministic mode<br/>(no Ollama, or `HIFZ_LLM_EVOLVE=false`) | LLM-augmented mode<br/>(Ollama reachable, `HIFZ_LLM_EVOLVE=true`) |
|---|---|---|
| **Insert pipeline** | Caller fields + regex file extraction + embed + kNN + co-occurrence edges | Same + single Ollama call producing keywords, tags, `context_summary`, typed semantic edges with reasons, and bounded neighbor evolution |
| **Edges produced** | `co_occurs_files`, `co_occurs_keywords`, `co_occurs_embedding`, `mentions`, `generated_by`, `informed_by`, `derived_from`, `part_of`, `follows`, `touches_file`, `commits_for`, `closes` / `supersedes` (when explicit) | All deterministic edges PLUS `broader`, `narrower`, `related`, `same_as`, `supports`, `contradicts`, `elaborates`, `responds_to`, `tests` |
| **`context_summary`** | Null | Populated by LLM |
| **`tags`** | Defaults to `[category-as-tag]` if empty | LLM-extended |
| **`evolution_history`** | Empty | Append-only audit of LLM rewrites to neighbor memories |
| **Cost** | One embedding call per insert | One embedding + one Ollama call per insert (>80 chars salience floor); neighbor rewrites async |
| **Quality bound** | Caller's keyword/file discipline + embedding model (fastembed AllMiniLM-L6-V2) | Above + LLM judgment quality |
| **Salience gate** | n/a | Memories with `content.len() < 80` skip the LLM call (fall back to deterministic) |
| **Observations** | Always deterministic only — never LLM-enriched (volume too high) | Same |
| **Consolidation (session end)** | Deterministic decay + uncommitted-runs filter | Above + LLM-based semantic / reflect / procedural tiers |
| **Cross-session warmup** | Deterministic project digest from typed categories | Same (warmup is read-only; doesn't depend on LLM) |

### Why two modes?

- **Deterministic mode** is predictable, fast, fully reproducible
  (benchmarks pin to it for that reason), and works in airgapped
  environments. It's enough to make hifz useful — typed categories +
  co-occurrence + provenance edges + the warmup digest cover the basics.
- **LLM-augmented mode** adds the semantic layer (typed conceptual /
  argumentative relations with rationales) that turns the graph from a
  "things that share tokens" graph into a knowledge graph an agent can
  walk meaningfully.

The same code path runs in both modes. Switching is a config flag —
rows written in one mode read fine in the other.

### Configuration

| Env var | Default | Effect |
|---|---|---|
| `OLLAMA_URL` | unset | When set, hifz attempts to reach Ollama at this URL. If unreachable, mode degrades to deterministic. |
| `OLLAMA_MODEL` | `qwen2.5:7b` | Model used for enrichment + evolve. |
| `HIFZ_LLM_EVOLVE` | `false` | Enables LLM enrichment + bounded neighbor evolution at insert time. Controls the `enable_llm` flag passed to `enrich::save_enriched`. |
| `HIFZ_INJECT_CONTEXT` | `true` | Claude Code adapter: inject the SessionStart warmup digest as system context. |
| `HIFZ_WARMUP_TOP_N` | `15` | Top-N entries in the warmup digest. |

---

## Examples

### Saving a typed memory

```bash
curl -XPOST http://localhost:3111/api/v1/memories -H 'Content-Type: application/json' -d '{
  "title": "JWT expiry race",
  "content": "Tokens expire 10s early due to clock skew between auth and api hosts.",
  "category": "bug",
  "keywords": ["jwt", "auth", "session"],
  "files": ["src/auth.rs", "src/session.rs"],
  "project": "hifz"
}'
```

Then a fix:

```bash
curl -XPOST http://localhost:3111/api/v1/memories -H 'Content-Type: application/json' -d '{
  "title": "Tolerate 30s clock skew on JWT verification",
  "content": "Adds a 30s leeway to the exp check.",
  "category": "fix",
  "keywords": ["jwt", "auth"],
  "files": ["src/auth.rs"],
  "closes_memory_id": "memory:abc",
  "project": "hifz"
}'
```

Result: a `closes` edge new→old; the bug no longer appears in
`/projects/hifz/accumulators` open_bugs.

### Long-form artifact

```bash
curl -XPOST http://localhost:3111/api/v1/memories -H 'Content-Type: application/json' -d '{
  "title": "Refactor auth flow — Q2 plan",
  "content": "Multi-step plan to centralize JWT validation and rate-limiting.",
  "content_long": "# Refactor auth flow\n\n## Step 1: extract validator\n... (full markdown body) ...",
  "category": "plan",
  "keywords": ["auth", "refactor"],
  "project": "hifz"
}'
```

`content_long` gets split into ~500-token chunks; each chunk gets its
own embedding + a `part_of` edge to the parent. Search hits chunks via
vector + BM25 and surfaces the parent with the matching chunk's content.

### Edit round-trip

```bash
hifz export --project hifz --out ./vault/
$EDITOR ./vault/memory_xyz.md
hifz import --from ./vault/
```

Each edit writes a new memory version that supersedes the old; the old
row's `is_latest` is set to false; a `supersedes` edge new→old is
written; chunks are regenerated.

### Typed graph walk

```bash
curl 'http://localhost:3111/api/v1/memories/memory:xyz/neighbors?relations=elaborates,related&max_hops=2'
```

Returns up to 50 neighbors traversing only conceptual edges, dampened
0.5× per hop.
