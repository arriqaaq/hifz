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
    for (i, stmt) in schema
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.starts_with("--"))
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
DEFINE FIELD IF NOT EXISTS tags              ON session TYPE option<array<string>>;
DEFINE INDEX IF NOT EXISTS session_project   ON TABLE session FIELDS project;
DEFINE INDEX IF NOT EXISTS session_status    ON TABLE session FIELDS status;

DEFINE TABLE IF NOT EXISTS observation SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id   ON observation TYPE record<session>;
DEFINE FIELD IF NOT EXISTS timestamp    ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS obs_type     ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS title        ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS subtitle     ON observation TYPE option<string>;
DEFINE FIELD IF NOT EXISTS facts        ON observation TYPE array<string>;
DEFINE FIELD IF NOT EXISTS facts_text   ON observation TYPE option<string>;
DEFINE FIELD IF NOT EXISTS narrative    ON observation TYPE string;
DEFINE FIELD IF NOT EXISTS concepts     ON observation TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files        ON observation TYPE array<string>;
DEFINE FIELD IF NOT EXISTS importance   ON observation TYPE int;
DEFINE FIELD IF NOT EXISTS confidence   ON observation TYPE option<float>;
DEFINE FIELD IF NOT EXISTS embedding    ON observation TYPE option<array<float>>;

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

DEFINE TABLE IF NOT EXISTS hifz SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project       ON hifz TYPE string DEFAULT 'global';
DEFINE FIELD IF NOT EXISTS mem_type      ON hifz TYPE string;
DEFINE FIELD IF NOT EXISTS title         ON hifz TYPE string;
DEFINE FIELD IF NOT EXISTS content       ON hifz TYPE string;
DEFINE FIELD IF NOT EXISTS concepts      ON hifz TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files         ON hifz TYPE array<string>;
DEFINE FIELD IF NOT EXISTS keywords      ON hifz TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS tags          ON hifz TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS context       ON hifz TYPE option<string>;
DEFINE FIELD IF NOT EXISTS session_ids   ON hifz TYPE array<record<session>>;
DEFINE FIELD IF NOT EXISTS strength      ON hifz TYPE float;
DEFINE FIELD IF NOT EXISTS access_count  ON hifz TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_accessed_at ON hifz TYPE string DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS embedding     ON hifz TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS version       ON hifz TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS parent_id     ON hifz TYPE option<record<hifz>>;
DEFINE FIELD IF NOT EXISTS supersedes    ON hifz TYPE option<array<record<hifz>>>;
DEFINE FIELD IF NOT EXISTS is_latest     ON hifz TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS forget_after  ON hifz TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at    ON hifz TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at    ON hifz TYPE string;
DEFINE INDEX IF NOT EXISTS mem_title_ft  ON TABLE hifz
  FIELDS title FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS mem_content_ft ON TABLE hifz
  FIELDS content FULLTEXT ANALYZER obs_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS mem_vec       ON TABLE hifz
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;
DEFINE INDEX IF NOT EXISTS mem_project   ON TABLE hifz FIELDS project;
DEFINE INDEX IF NOT EXISTS mem_latest    ON TABLE hifz FIELDS is_latest;

DEFINE TABLE IF NOT EXISTS summary SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id        ON summary TYPE record<session>;
DEFINE FIELD IF NOT EXISTS project           ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS created_at        ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS title             ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS narrative         ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS key_decisions     ON summary TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files_modified    ON summary TYPE array<string>;
DEFINE FIELD IF NOT EXISTS concepts          ON summary TYPE array<string>;
DEFINE FIELD IF NOT EXISTS observation_count ON summary TYPE int;

-- === CONSOLIDATION TIERS ===

DEFINE TABLE IF NOT EXISTS semantic_hifz SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS fact              ON semantic_hifz TYPE string;
DEFINE FIELD IF NOT EXISTS confidence        ON semantic_hifz TYPE float;
DEFINE FIELD IF NOT EXISTS source_sessions   ON semantic_hifz TYPE array<record<session>>;
DEFINE FIELD IF NOT EXISTS access_count      ON semantic_hifz TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS strength          ON semantic_hifz TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS last_accessed_at  ON semantic_hifz TYPE string;
DEFINE FIELD IF NOT EXISTS created_at        ON semantic_hifz TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at        ON semantic_hifz TYPE string;

-- === CORE MEMORY (MemGPT-style always-on block) ===
-- Per-project singleton. `id` is deterministic: hifz_core:<project-slug>.
DEFINE TABLE IF NOT EXISTS hifz_core SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project     ON hifz_core TYPE string;
DEFINE FIELD IF NOT EXISTS identity    ON hifz_core TYPE option<string>;
DEFINE FIELD IF NOT EXISTS goals       ON hifz_core TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS invariants  ON hifz_core TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS watchlist   ON hifz_core TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS updated_at  ON hifz_core TYPE string;
DEFINE INDEX IF NOT EXISTS core_project ON TABLE hifz_core FIELDS project UNIQUE;

-- === ENTITIES (Phase 4) ===
-- Typed named things (files, symbols, concepts, errors) mentioned across
-- observations and memories. Used to bridge memories sharing a topic.
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS kind       ON entity TYPE string;  -- file|symbol|concept|error
DEFINE FIELD IF NOT EXISTS name       ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS project    ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS first_seen ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS last_seen  ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS count      ON entity TYPE int DEFAULT 1;
DEFINE INDEX IF NOT EXISTS entity_unique ON TABLE entity FIELDS kind, name, project UNIQUE;

-- === EPISODES (Phase 4) ===
-- A task-scoped trajectory inside a session: UserPromptSubmit → … → Stop/TaskCompleted.
DEFINE TABLE IF NOT EXISTS episode SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id      ON episode TYPE record<session>;
DEFINE FIELD IF NOT EXISTS project         ON episode TYPE string;
DEFINE FIELD IF NOT EXISTS started_at      ON episode TYPE string;
DEFINE FIELD IF NOT EXISTS ended_at        ON episode TYPE option<string>;
DEFINE FIELD IF NOT EXISTS prompt          ON episode TYPE string;
DEFINE FIELD IF NOT EXISTS outcome         ON episode TYPE string DEFAULT 'unknown';
DEFINE FIELD IF NOT EXISTS observation_ids ON episode TYPE array<record<observation>> DEFAULT [];
DEFINE FIELD IF NOT EXISTS lesson          ON episode TYPE option<string>;
DEFINE INDEX IF NOT EXISTS ep_project ON TABLE episode FIELDS project;
DEFINE INDEX IF NOT EXISTS ep_session ON TABLE episode FIELDS session_id;
DEFINE ANALYZER IF NOT EXISTS ep_analyzer TOKENIZERS blank, class
  FILTERS lowercase, snowball(english);
DEFINE INDEX IF NOT EXISTS ep_prompt_ft ON TABLE episode
  FIELDS prompt FULLTEXT ANALYZER ep_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS ep_lesson_ft ON TABLE episode
  FIELDS lesson FULLTEXT ANALYZER ep_analyzer BM25 CONCURRENTLY;

-- === GRAPH LINKS BETWEEN MEMORIES (Phase 3) ===
-- Typed relation edge. `via` distinguishes the reason two memories are linked:
--   embedding  — KNN cosine similarity
--   concept    — Jaccard overlap on concepts
--   file       — Jaccard overlap on files
--   entity     — shared entity mention (Phase 4)
--   semantic   — proposed by evolution (Phase 5, LLM)
-- NOTE: RELATE ... UNIQUE enforces (in, out) uniqueness only. Per-via dedup is
-- handled Rust-side in src/link.rs before any RELATE call.
DEFINE TABLE IF NOT EXISTS mem_link SCHEMAFULL TYPE RELATION IN hifz OUT hifz;
DEFINE FIELD IF NOT EXISTS score      ON mem_link TYPE float;
DEFINE FIELD IF NOT EXISTS via        ON mem_link TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON mem_link TYPE string;
DEFINE INDEX IF NOT EXISTS mem_link_via ON TABLE mem_link FIELDS via;

DEFINE TABLE IF NOT EXISTS procedural_hifz SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name              ON procedural_hifz TYPE string;
DEFINE FIELD IF NOT EXISTS steps             ON procedural_hifz TYPE array<string>;
DEFINE FIELD IF NOT EXISTS trigger_condition ON procedural_hifz TYPE string;
DEFINE FIELD IF NOT EXISTS frequency         ON procedural_hifz TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS strength          ON procedural_hifz TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS source_sessions   ON procedural_hifz TYPE array<record<session>>;
DEFINE FIELD IF NOT EXISTS created_at        ON procedural_hifz TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at        ON procedural_hifz TYPE string;

"#;
