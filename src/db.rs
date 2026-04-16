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
DEFINE FIELD IF NOT EXISTS mem_type      ON hifz TYPE string;
DEFINE FIELD IF NOT EXISTS title         ON hifz TYPE string;
DEFINE FIELD IF NOT EXISTS content       ON hifz TYPE string;
DEFINE FIELD IF NOT EXISTS concepts      ON hifz TYPE array<string>;
DEFINE FIELD IF NOT EXISTS files         ON hifz TYPE array<string>;
DEFINE FIELD IF NOT EXISTS session_ids   ON hifz TYPE array<record<session>>;
DEFINE FIELD IF NOT EXISTS strength      ON hifz TYPE float;
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
