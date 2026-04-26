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
DEFINE FIELD IF NOT EXISTS name              ON session TYPE option<string>;
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
DEFINE FIELD IF NOT EXISTS keywords     ON observation TYPE array<string>;
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

DEFINE TABLE IF NOT EXISTS memory SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project          ON memory TYPE string DEFAULT 'global';
DEFINE FIELD IF NOT EXISTS category         ON memory TYPE string;
DEFINE FIELD IF NOT EXISTS title            ON memory TYPE string;
DEFINE FIELD IF NOT EXISTS content          ON memory TYPE string;
DEFINE FIELD IF NOT EXISTS keywords         ON memory TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files            ON memory TYPE array<string>;
DEFINE FIELD IF NOT EXISTS tags             ON memory TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS context          ON memory TYPE option<string>;
DEFINE FIELD IF NOT EXISTS strength         ON memory TYPE float;
DEFINE FIELD IF NOT EXISTS retrieval_count  ON memory TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_accessed_at ON memory TYPE string DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS embedding        ON memory TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS version          ON memory TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS parent_id        ON memory TYPE option<record<memory>>;
DEFINE FIELD IF NOT EXISTS supersedes       ON memory TYPE option<array<record<memory>>>;
DEFINE FIELD IF NOT EXISTS is_latest        ON memory TYPE bool DEFAULT true;
DEFINE FIELD IF NOT EXISTS forget_after     ON memory TYPE option<string>;
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

DEFINE TABLE IF NOT EXISTS summary SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS session_id        ON summary TYPE record<session>;
DEFINE FIELD IF NOT EXISTS project           ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS created_at        ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS title             ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS narrative         ON summary TYPE string;
DEFINE FIELD IF NOT EXISTS key_decisions     ON summary TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files_modified    ON summary TYPE array<string>;
DEFINE FIELD IF NOT EXISTS keywords          ON summary TYPE array<string>;
DEFINE FIELD IF NOT EXISTS observation_count ON summary TYPE int;

-- === CONSOLIDATION TIERS ===

DEFINE TABLE IF NOT EXISTS semantic_memory SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS fact              ON semantic_memory TYPE string;
DEFINE FIELD IF NOT EXISTS confidence        ON semantic_memory TYPE float;
DEFINE FIELD IF NOT EXISTS source_sessions   ON semantic_memory TYPE array<record<session>>;
DEFINE FIELD IF NOT EXISTS retrieval_count   ON semantic_memory TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS strength          ON semantic_memory TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS last_accessed_at  ON semantic_memory TYPE string;
DEFINE FIELD IF NOT EXISTS created_at        ON semantic_memory TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at        ON semantic_memory TYPE string;

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

-- === ENTITIES ===
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS kind       ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS name       ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS project    ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS first_seen ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS last_seen  ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS count      ON entity TYPE int DEFAULT 1;
DEFINE INDEX IF NOT EXISTS entity_unique ON TABLE entity FIELDS kind, name, project UNIQUE;

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
DEFINE FIELD IF NOT EXISTS commit_id       ON run TYPE option<record<commit>>;
DEFINE FIELD IF NOT EXISTS plan_id         ON run TYPE option<record<plan>>;
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
-- Relation types (derived_from, informed, similar_to, etc.) are prescribed
-- by application-level enums; unknown strings accepted for extensibility.
DEFINE TABLE IF NOT EXISTS edge SCHEMAFULL TYPE RELATION;
DEFINE FIELD IF NOT EXISTS relation   ON edge TYPE string;
DEFINE FIELD IF NOT EXISTS via        ON edge TYPE string;
DEFINE FIELD IF NOT EXISTS score      ON edge TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS metadata   ON edge TYPE option<object>;
DEFINE FIELD IF NOT EXISTS created_at ON edge TYPE string;
DEFINE INDEX IF NOT EXISTS edge_relation ON TABLE edge FIELDS relation;
DEFINE INDEX IF NOT EXISTS edge_via      ON TABLE edge FIELDS via;
DEFINE INDEX IF NOT EXISTS edge_in       ON TABLE edge FIELDS in;
DEFINE INDEX IF NOT EXISTS edge_out      ON TABLE edge FIELDS out;

-- === COMMIT TRACKING ===
DEFINE TABLE IF NOT EXISTS commit SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS sha            ON commit TYPE string;
DEFINE FIELD IF NOT EXISTS message        ON commit TYPE string;
DEFINE FIELD IF NOT EXISTS author         ON commit TYPE string;
DEFINE FIELD IF NOT EXISTS branch         ON commit TYPE string;
DEFINE FIELD IF NOT EXISTS project        ON commit TYPE string;
DEFINE FIELD IF NOT EXISTS files_changed  ON commit TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS insertions     ON commit TYPE option<int>;
DEFINE FIELD IF NOT EXISTS deletions      ON commit TYPE option<int>;
DEFINE FIELD IF NOT EXISTS is_amend       ON commit TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS session_id     ON commit TYPE option<record<session>>;
DEFINE FIELD IF NOT EXISTS run_id         ON commit TYPE option<record<run>>;
DEFINE FIELD IF NOT EXISTS plan_id        ON commit TYPE option<record<plan>>;
DEFINE FIELD IF NOT EXISTS timestamp      ON commit TYPE string;
DEFINE FIELD IF NOT EXISTS created_at     ON commit TYPE string;
DEFINE INDEX IF NOT EXISTS commit_sha     ON TABLE commit FIELDS sha UNIQUE;
DEFINE INDEX IF NOT EXISTS commit_project ON TABLE commit FIELDS project;

-- === PLAN TRACKING ===
DEFINE TABLE IF NOT EXISTS plan SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS file_path      ON plan TYPE string;
DEFINE FIELD IF NOT EXISTS title          ON plan TYPE string;
DEFINE FIELD IF NOT EXISTS content        ON plan TYPE string;
DEFINE FIELD IF NOT EXISTS status         ON plan TYPE string DEFAULT 'active';
DEFINE FIELD IF NOT EXISTS project        ON plan TYPE string;
DEFINE FIELD IF NOT EXISTS keywords       ON plan TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS files          ON plan TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS session_id     ON plan TYPE option<record<session>>;
DEFINE FIELD IF NOT EXISTS commit_id      ON plan TYPE option<record<commit>>;
DEFINE FIELD IF NOT EXISTS created_at     ON plan TYPE string;
DEFINE FIELD IF NOT EXISTS completed_at   ON plan TYPE option<string>;
DEFINE INDEX IF NOT EXISTS plan_project   ON TABLE plan FIELDS project;
DEFINE INDEX IF NOT EXISTS plan_status    ON TABLE plan FIELDS status;
DEFINE INDEX IF NOT EXISTS plan_file_path ON TABLE plan FIELDS file_path UNIQUE;

DEFINE TABLE IF NOT EXISTS procedural_memory SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name              ON procedural_memory TYPE string;
DEFINE FIELD IF NOT EXISTS steps             ON procedural_memory TYPE array<string>;
DEFINE FIELD IF NOT EXISTS trigger_condition ON procedural_memory TYPE string;
DEFINE FIELD IF NOT EXISTS frequency         ON procedural_memory TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS strength          ON procedural_memory TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS source_sessions   ON procedural_memory TYPE array<record<session>>;
DEFINE FIELD IF NOT EXISTS created_at        ON procedural_memory TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at        ON procedural_memory TYPE string;

"#;
