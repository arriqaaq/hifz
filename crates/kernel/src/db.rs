use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::engine::local::{Mem, SurrealKv};

pub type Db = surrealdb::engine::local::Db;

/// Connect to persistent SurrealDB at the given path.
pub async fn connect(path: &str) -> Result<Surreal<Db>> {
    let db = Surreal::new::<SurrealKv>(path).await?;
    db.use_ns("hifz").use_db("main").await?;
    Ok(db)
}

/// Connect to in-memory SurrealDB (ephemeral, data lost on restart).
pub async fn connect_mem() -> Result<Surreal<Db>> {
    let db = Surreal::new::<Mem>(()).await?;
    db.use_ns("hifz").use_db("main").await?;
    Ok(db)
}

/// Initialize the database schema.
pub async fn init_schema(db: &Surreal<Db>, embed_dim: usize) -> Result<()> {
    let schema = SCHEMA.replace("DIMENSION 384", &format!("DIMENSION {embed_dim}"));
    // Strip line-comments (`-- ...`) before splitting by `;`. The trailing
    // inline-comment idiom (`DEFINE FIELD foo ON x TYPE string; -- denorm`)
    // would otherwise cause split-by-`;` to produce a chunk that *starts*
    // with `--`, and the empty-string filter below would silently drop the
    // following statement.
    let stripped: String = schema
        .lines()
        .map(|line| match line.find("--") {
            Some(idx) => &line[..idx],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for (i, stmt) in stripped
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .enumerate()
    {
        let sql = format!("{stmt};");
        if let Err(e) = db.query(&sql).await.and_then(|r| r.check()) {
            tracing::error!(
                "Schema statement {i} failed: {e}\n  SQL: {}",
                stmt.chars().take(120).collect::<String>()
            );
            return Err(e.into());
        }
    }
    tracing::info!("Database schema initialized");
    Ok(())
}

const SCHEMA: &str = r#"
-- === CORE TABLES ===

DEFINE TABLE IF NOT EXISTS session SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project           ON session TYPE string;
DEFINE FIELD IF NOT EXISTS cwd               ON session TYPE string;
DEFINE FIELD IF NOT EXISTS started_at        ON session TYPE string;
DEFINE FIELD IF NOT EXISTS ended_at          ON session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS status            ON session TYPE string;
DEFINE FIELD IF NOT EXISTS observation_count ON session TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS model             ON session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS name              ON session TYPE option<string>;
DEFINE FIELD IF NOT EXISTS tags              ON session TYPE option<array<string>>;
DEFINE INDEX IF NOT EXISTS session_project   ON TABLE session FIELDS project;
DEFINE INDEX IF NOT EXISTS session_status    ON TABLE session FIELDS status;

DEFINE TABLE IF NOT EXISTS observation SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id    ON observation TYPE record<session>;
DEFINE FIELD IF NOT EXISTS ord           ON observation TYPE int;
DEFINE FIELD IF NOT EXISTS parent_obs_id ON observation TYPE option<record<observation>>;
DEFINE FIELD IF NOT EXISTS source        ON observation TYPE string DEFAULT 'hook';
DEFINE FIELD IF NOT EXISTS timestamp     ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS obs_type      ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS title         ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS subtitle      ON observation TYPE option<string>;
DEFINE FIELD IF NOT EXISTS facts         ON observation TYPE array<string>;
DEFINE FIELD IF NOT EXISTS facts_text    ON observation TYPE option<string>;
DEFINE FIELD IF NOT EXISTS narrative     ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS keywords      ON observation TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files         ON observation TYPE array<string>;
DEFINE FIELD IF NOT EXISTS importance    ON observation TYPE int;
DEFINE FIELD IF NOT EXISTS confidence    ON observation TYPE option<float>;
DEFINE FIELD IF NOT EXISTS embedding     ON observation TYPE option<array<float>>;
-- FLEXIBLE: adapter-supplied `metadata` carries arbitrary nested keys
-- (e.g. a commit_made's {sha, branch, message, file_status, is_revert}).
-- Without FLEXIBLE, SCHEMAFULL rejects any record with nested metadata
-- keys. OVERWRITE (not IF NOT EXISTS) so DBs created before this field
-- was ever written are widened in place — the column was always empty
-- (HookPayload dropped adapter metadata pre-Phase-1), so this is lossless.
DEFINE FIELD OVERWRITE metadata      ON observation TYPE option<object> FLEXIBLE;

DEFINE ANALYZER IF NOT EXISTS obs_analyzer TOKENIZERS blank, class
  FILTERS lowercase, snowball(english);
DEFINE INDEX IF NOT EXISTS obs_title_ft     ON TABLE observation
  FIELDS title FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS obs_narrative_ft ON TABLE observation
  FIELDS narrative FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS obs_facts_ft     ON TABLE observation
  FIELDS facts_text FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS obs_vec          ON TABLE observation
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;
DEFINE INDEX IF NOT EXISTS obs_session      ON TABLE observation FIELDS session_id;
DEFINE INDEX IF NOT EXISTS obs_type         ON TABLE observation FIELDS obs_type;
DEFINE INDEX IF NOT EXISTS obs_session_ord  ON TABLE observation FIELDS session_id, ord UNIQUE;
DEFINE INDEX IF NOT EXISTS obs_parent       ON TABLE observation FIELDS parent_obs_id;
DEFINE INDEX IF NOT EXISTS obs_source       ON TABLE observation FIELDS source;

DEFINE TABLE IF NOT EXISTS memory SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project          ON memory TYPE string DEFAULT 'global';
DEFINE FIELD IF NOT EXISTS category         ON memory TYPE string DEFAULT 'note';
DEFINE FIELD IF NOT EXISTS title            ON memory TYPE string;
DEFINE FIELD IF NOT EXISTS content          ON memory TYPE string;
DEFINE FIELD IF NOT EXISTS content_long     ON memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS keywords         ON memory TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files            ON memory TYPE array<string>;
DEFINE FIELD IF NOT EXISTS tags             ON memory TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS context          ON memory TYPE option<string>;
-- Phase 2: A-MEM-style insert pipeline adds these.
-- context_summary is the LLM-generated paragraph placing this memory in
-- the broader work. evolution_history is an append-only audit trail of
-- LLM rewrites applied during bounded neighbor evolution.
DEFINE FIELD IF NOT EXISTS context_summary  ON memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS evolution_history ON memory TYPE array<object> DEFAULT [];
DEFINE FIELD IF NOT EXISTS strength         ON memory TYPE float;
DEFINE FIELD IF NOT EXISTS retrieval_count  ON memory TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_accessed_at ON memory TYPE string DEFAULT <string>time::now();
DEFINE FIELD IF NOT EXISTS embedding        ON memory TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS version          ON memory TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS parent_id        ON memory TYPE option<record<memory>>;
DEFINE FIELD IF NOT EXISTS supersedes       ON memory TYPE option<array<record<memory>>>;
DEFINE FIELD IF NOT EXISTS is_latest        ON memory TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS pinned           ON memory TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS forget_after     ON memory TYPE option<string>;
-- Outcome-grounding watermark: RFC3339 timestamp of the last commit that
-- touched this memory's files (any origin: Claude tool, terminal, PR-pull,
-- rebase). Mirrors `code_file.last_committed_at`. Memories with this set are
-- exempt from disuse decay / weak-strength GC (committed work persists).
-- option<string>, defaults to NONE — no backfill required.
DEFINE FIELD IF NOT EXISTS last_committed_at ON memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at       ON memory TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at       ON memory TYPE string;
DEFINE INDEX IF NOT EXISTS mem_title_ft     ON TABLE memory
  FIELDS title FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS mem_content_ft   ON TABLE memory
  FIELDS content FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS mem_vec          ON TABLE memory
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;
DEFINE INDEX IF NOT EXISTS mem_project      ON TABLE memory FIELDS project;
DEFINE INDEX IF NOT EXISTS mem_latest       ON TABLE memory FIELDS is_latest;

-- === MEMORY CHUNKS (Phase 4: long-form artifact retrieval) ===
-- For long-form categories (Plan, Design, CodeReview, ShipReport,
-- ContextSlice), `Memory.content_long` is split into ~500-token chunks with
-- 100-token overlap. Each chunk is its own row with its own embedding so
-- search can retrieve the relevant section of a 50KB document. Chunks link
-- back to the parent via the `edge` table (`relation='part_of'`).
DEFINE TABLE IF NOT EXISTS memory_chunk SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS parent_id   ON memory_chunk TYPE record<memory>;
DEFINE FIELD IF NOT EXISTS project     ON memory_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS chunk_index ON memory_chunk TYPE int;
DEFINE FIELD IF NOT EXISTS content     ON memory_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS embedding   ON memory_chunk TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS created_at  ON memory_chunk TYPE string;
DEFINE INDEX IF NOT EXISTS chunk_parent  ON TABLE memory_chunk FIELDS parent_id;
DEFINE INDEX IF NOT EXISTS chunk_project ON TABLE memory_chunk FIELDS project;
DEFINE INDEX IF NOT EXISTS chunk_content_ft ON TABLE memory_chunk
  FIELDS content FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS chunk_vec      ON TABLE memory_chunk
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;

-- === CORE MEMORY (MemGPT-style always-on block) ===
-- Per-project singleton.
DEFINE TABLE IF NOT EXISTS core_memory SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project     ON core_memory TYPE string;
DEFINE FIELD IF NOT EXISTS identity    ON core_memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS goals       ON core_memory TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS invariants  ON core_memory TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS watchlist   ON core_memory TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS updated_at  ON core_memory TYPE string;
DEFINE INDEX IF NOT EXISTS core_project ON TABLE core_memory FIELDS project UNIQUE;

-- === RUNS ===
DEFINE TABLE IF NOT EXISTS run SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id      ON run TYPE record<session>;
DEFINE FIELD IF NOT EXISTS project         ON run TYPE string;
DEFINE FIELD IF NOT EXISTS started_at      ON run TYPE string;
DEFINE FIELD IF NOT EXISTS ended_at        ON run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS prompt          ON run TYPE string;
DEFINE FIELD IF NOT EXISTS prompts         ON run TYPE option<array<string>>;
DEFINE FIELD IF NOT EXISTS outcome         ON run TYPE string DEFAULT 'unknown';
DEFINE FIELD IF NOT EXISTS observation_ids ON run TYPE array<record<observation>> DEFAULT [];
DEFINE FIELD IF NOT EXISTS lesson          ON run TYPE option<string>;
DEFINE FIELD IF NOT EXISTS recalled_ids    ON run TYPE array<record<memory>> DEFAULT [];
DEFINE INDEX IF NOT EXISTS run_project ON TABLE run FIELDS project;
DEFINE INDEX IF NOT EXISTS run_session ON TABLE run FIELDS session_id;
DEFINE ANALYZER IF NOT EXISTS run_analyzer TOKENIZERS blank, class
  FILTERS lowercase, snowball(english);
DEFINE INDEX IF NOT EXISTS run_prompt_ft ON TABLE run
  FIELDS prompt FULLTEXT ANALYZER run_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS run_lesson_ft ON TABLE run
  FIELDS lesson FULLTEXT ANALYZER run_analyzer BM25 CONCURRENTLY;

-- === KNOWLEDGE GRAPH EDGES ===
-- Generic relation table: any record type can be an endpoint.
-- Relation types are prescribed by `models::EdgeRelation` (typed groups:
-- co-occurrence / provenance / conceptual / argumentative / lifecycle /
-- code-domain). Per-relation type-pair constraints enforced at write time
-- in `link::is_allowed_relation`.
-- `reason` is a first-class one-line justification: deterministic edges
-- record the channel + score (e.g. "keyword overlap: jwt, auth (2/5)"),
-- LLM-set edges record the model's one-sentence rationale.
DEFINE TABLE IF NOT EXISTS edge SCHEMAFULL TYPE RELATION;
DEFINE FIELD IF NOT EXISTS relation   ON edge TYPE string;
DEFINE FIELD IF NOT EXISTS via        ON edge TYPE string;
DEFINE FIELD IF NOT EXISTS score      ON edge TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS reason     ON edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS metadata   ON edge TYPE option<object>;
DEFINE FIELD IF NOT EXISTS created_at ON edge TYPE string;
-- Phase 0b: code-edge resolution provenance — resolved|external|ambiguous.
-- Optional (legacy edges have none); set by the code-intel core (E4).
DEFINE FIELD IF NOT EXISTS resolution ON edge TYPE option<string>;
DEFINE INDEX IF NOT EXISTS edge_relation ON TABLE edge FIELDS relation;
DEFINE INDEX IF NOT EXISTS edge_via      ON TABLE edge FIELDS via;
DEFINE INDEX IF NOT EXISTS edge_in       ON TABLE edge FIELDS in;
DEFINE INDEX IF NOT EXISTS edge_out      ON TABLE edge FIELDS out;

-- === CODE INDEX TABLES (M1+) ===
-- Native-Rust port of cocoindex-code's chunk + index pipeline. Files are walked
-- (gitignore-honest), chunked language-aware via tree-sitter, embedded, and
-- searched via HNSW + BM25. Memories cross-link to chunks via the `references`
-- edge and to named symbols via `references_symbol`.
--
-- Re-indexing is idempotent: `code_file.mtime_ns` + `content_hash` short-circuit
-- unchanged files. When a file changes, `re_anchor_references` rewrites edges
-- from old chunks to new chunks that overlap the same line range — making the
-- "memory references a precise point in code" graph durable across edits.

DEFINE TABLE IF NOT EXISTS code_file SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project            ON code_file TYPE string;
DEFINE FIELD IF NOT EXISTS path               ON code_file TYPE string;            -- repo-relative POSIX
DEFINE FIELD IF NOT EXISTS abs_path           ON code_file TYPE string;
DEFINE FIELD IF NOT EXISTS language           ON code_file TYPE string;
DEFINE FIELD IF NOT EXISTS size_bytes         ON code_file TYPE int;
DEFINE FIELD IF NOT EXISTS mtime_ns           ON code_file TYPE int;
DEFINE FIELD IF NOT EXISTS content_hash       ON code_file TYPE string;            -- sha256 hex
DEFINE FIELD IF NOT EXISTS chunk_count        ON code_file TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_referenced_at ON code_file TYPE option<string>;
DEFINE FIELD IF NOT EXISTS last_committed_at  ON code_file TYPE option<string>;
DEFINE FIELD IF NOT EXISTS deleted_at         ON code_file TYPE option<string>;    -- tombstone, swept after 30d
DEFINE FIELD IF NOT EXISTS indexed_at         ON code_file TYPE string;
DEFINE INDEX IF NOT EXISTS code_file_unique ON TABLE code_file FIELDS project, path UNIQUE;
DEFINE INDEX IF NOT EXISTS code_file_lang   ON TABLE code_file FIELDS language;

DEFINE TABLE IF NOT EXISTS code_chunk SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS file               ON code_chunk TYPE record<code_file>;
DEFINE FIELD IF NOT EXISTS project            ON code_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS path               ON code_chunk TYPE string;            -- denorm for filtering
DEFINE FIELD IF NOT EXISTS language           ON code_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS chunk_index        ON code_chunk TYPE int;
DEFINE FIELD IF NOT EXISTS content            ON code_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS start_line         ON code_chunk TYPE int;                -- 1-indexed inclusive
DEFINE FIELD IF NOT EXISTS end_line           ON code_chunk TYPE int;                -- 1-indexed inclusive
DEFINE FIELD IF NOT EXISTS start_byte         ON code_chunk TYPE int;
DEFINE FIELD IF NOT EXISTS end_byte           ON code_chunk TYPE int;
DEFINE FIELD IF NOT EXISTS content_hash       ON code_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS embedding          ON code_chunk TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS symbols            ON code_chunk TYPE array<string> DEFAULT [];  -- qualified names defined here
DEFINE FIELD IF NOT EXISTS strength           ON code_chunk TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS last_referenced_at ON code_chunk TYPE string DEFAULT <string>time::now();
DEFINE FIELD IF NOT EXISTS created_at         ON code_chunk TYPE string;
DEFINE INDEX IF NOT EXISTS code_chunk_file    ON TABLE code_chunk FIELDS file;
DEFINE INDEX IF NOT EXISTS code_chunk_project ON TABLE code_chunk FIELDS project;
DEFINE INDEX IF NOT EXISTS code_chunk_path    ON TABLE code_chunk FIELDS path;
DEFINE INDEX IF NOT EXISTS code_chunk_lang    ON TABLE code_chunk FIELDS language;
DEFINE INDEX IF NOT EXISTS code_chunk_lines   ON TABLE code_chunk FIELDS path, start_line;

-- Code analyzer: NO snowball stemming — identifiers like getUserId must not be
-- reduced to "getuserid". Memory text continues to use obs_analyzer.
DEFINE ANALYZER IF NOT EXISTS code_analyzer TOKENIZERS blank, class FILTERS lowercase;
DEFINE INDEX IF NOT EXISTS code_chunk_content_ft ON TABLE code_chunk
  FIELDS content FULLTEXT ANALYZER code_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS code_chunk_vec ON TABLE code_chunk
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;

-- Symbol-level cross-linking: a memory like "the parse_chunk function" survives
-- chunk re-splitting and reformatting because it points at the named symbol,
-- not a line range. Auto-linked from prose only for qualified patterns
-- (module::name) — bareword identifiers don't auto-link by design.
DEFINE TABLE IF NOT EXISTS code_symbol SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project            ON code_symbol TYPE string;
DEFINE FIELD IF NOT EXISTS name               ON code_symbol TYPE string;
DEFINE FIELD IF NOT EXISTS qualified          ON code_symbol TYPE string;            -- "chunk::parse_chunk"
DEFINE FIELD IF NOT EXISTS kind               ON code_symbol TYPE string;            -- function|struct|enum|trait|method|const|module|class|interface
DEFINE FIELD IF NOT EXISTS language           ON code_symbol TYPE string;
DEFINE FIELD IF NOT EXISTS file               ON code_symbol TYPE record<code_file>;
DEFINE FIELD IF NOT EXISTS path               ON code_symbol TYPE string;            -- denorm
DEFINE FIELD IF NOT EXISTS start_line         ON code_symbol TYPE int;
DEFINE FIELD IF NOT EXISTS end_line           ON code_symbol TYPE int;
DEFINE FIELD IF NOT EXISTS signature          ON code_symbol TYPE option<string>;
DEFINE FIELD IF NOT EXISTS doc                ON code_symbol TYPE option<string>;
DEFINE FIELD IF NOT EXISTS embedding          ON code_symbol TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS last_referenced_at ON code_symbol TYPE string DEFAULT <string>time::now();
DEFINE FIELD IF NOT EXISTS created_at         ON code_symbol TYPE string;
-- Phase 0b additive superset (optional → old .scm extractor still valid under
-- SCHEMAFULL; populated by the code-intel core in E4):
DEFINE FIELD IF NOT EXISTS start_byte         ON code_symbol TYPE option<int>;
DEFINE FIELD IF NOT EXISTS end_byte           ON code_symbol TYPE option<int>;
DEFINE FIELD IF NOT EXISTS parent_symbol      ON code_symbol TYPE option<record<code_symbol>>;  -- explicit containment
DEFINE FIELD IF NOT EXISTS body_hash          ON code_symbol TYPE option<string>;               -- structural rename detection
DEFINE FIELD IF NOT EXISTS visibility         ON code_symbol TYPE option<string>;               -- pub|exported|private
DEFINE FIELD IF NOT EXISTS chunk_span         ON code_symbol TYPE option<array<record<code_chunk>>> DEFAULT [];  -- replaces primary_chunk
-- E4: semantic qualified path is globally unique within a project (the
-- code-intel core guarantees it), so identity is `(project, qualified)`
-- UNIQUE → deterministic stable-id UPSERT, no wipe-recreate. `primary_chunk`
-- dropped (superseded by `chunk_span`).
DEFINE INDEX IF NOT EXISTS code_symbol_lookup ON TABLE code_symbol FIELDS project, qualified UNIQUE;
DEFINE INDEX IF NOT EXISTS code_symbol_name   ON TABLE code_symbol FIELDS project, name;
DEFINE INDEX IF NOT EXISTS code_symbol_kind   ON TABLE code_symbol FIELDS project, kind;
DEFINE INDEX IF NOT EXISTS code_symbol_vec    ON TABLE code_symbol
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;

-- Phase 0b: explicit node for call/import targets outside the indexed set
-- (stdlib / third-party / dynamic). Keyed by canonical import path so the
-- same external is one node. Unused until E4 wires the resolver.
DEFINE TABLE IF NOT EXISTS external_symbol SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project    ON external_symbol TYPE string;
DEFINE FIELD IF NOT EXISTS canonical  ON external_symbol TYPE string;   -- e.g. "std::fmt::Display", "react#useState"
DEFINE FIELD IF NOT EXISTS language   ON external_symbol TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON external_symbol TYPE string;
DEFINE INDEX IF NOT EXISTS external_symbol_key ON TABLE external_symbol FIELDS project, canonical UNIQUE;

-- === AGENT USAGE (generic, adapter-populated) ===
-- One row per LLM inference call. Vendor-neutral: adapters map their own
-- token-category fields into `breakdown` (a JSON object). The Claude Code
-- adapter, for example, fills breakdown.cache_read and breakdown.cache_creation;
-- a future OpenAI adapter would fill breakdown.cached_prompt. Top-level
-- input/output/total cover the universal case.

DEFINE TABLE IF NOT EXISTS agent_usage SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id    ON agent_usage TYPE record<session>;
DEFINE FIELD IF NOT EXISTS project       ON agent_usage TYPE string;
DEFINE FIELD IF NOT EXISTS agent         ON agent_usage TYPE string;
DEFINE FIELD IF NOT EXISTS provider      ON agent_usage TYPE option<string>;
DEFINE FIELD IF NOT EXISTS model         ON agent_usage TYPE string;
DEFINE FIELD IF NOT EXISTS external_id   ON agent_usage TYPE string;
DEFINE FIELD IF NOT EXISTS timestamp     ON agent_usage TYPE string;
DEFINE FIELD IF NOT EXISTS input_tokens  ON agent_usage TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS output_tokens ON agent_usage TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS total_tokens  ON agent_usage TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS prompt        ON agent_usage TYPE option<string>;
DEFINE FIELD IF NOT EXISTS prompt_at     ON agent_usage TYPE option<string>;
DEFINE FIELD IF NOT EXISTS tools         ON agent_usage TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS run_id        ON agent_usage TYPE option<record<run>>;
-- FLEXIBLE: `breakdown` holds adapter-defined keys (cache_read,
-- cache_creation, ...) that aren't declared on this SCHEMAFULL table.
-- Without FLEXIBLE, SurrealDB rejects every record that carries a
-- breakdown — i.e. every cache-using Claude Code call.
DEFINE FIELD IF NOT EXISTS breakdown     ON agent_usage TYPE option<object> FLEXIBLE;
-- Per-file count of auxiliary Anthropic calls (ai-title, summary) that were
-- billed but excluded from the JSONL transcript. The adapter stamps this on
-- the FIRST emitted record of each file; the (agent, external_id) UNIQUE
-- index makes re-ingestion of the same file idempotent.
DEFINE FIELD IF NOT EXISTS aux_calls     ON agent_usage TYPE option<int>;
DEFINE INDEX IF NOT EXISTS au_ext_uniq   ON TABLE agent_usage FIELDS agent, external_id UNIQUE;
DEFINE INDEX IF NOT EXISTS au_session    ON TABLE agent_usage FIELDS session_id;
DEFINE INDEX IF NOT EXISTS au_project    ON TABLE agent_usage FIELDS project;
DEFINE INDEX IF NOT EXISTS au_ts         ON TABLE agent_usage FIELDS timestamp;
DEFINE INDEX IF NOT EXISTS au_model      ON TABLE agent_usage FIELDS model;

"#;
