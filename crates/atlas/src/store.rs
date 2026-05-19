//! atlas SurrealDB schema + connection.
//!
//! atlas tables live in the **same** SurrealKV instance as hifz (so the
//! corpus graph can later cross-link to hifz's grounded code/memory graph)
//! but are fully isolated: `atlas_*`, never touching hifz tables. Schema
//! init mirrors `kernel::db::init_schema` (strip `--` comments, split
//! by `;`, run each statement, fail loudly).

use anyhow::Result;
use kernel::db::Db;
use surrealdb::Surreal;

/// Thin handle: the shared connection + the project the CLI/REST operate on.
pub struct Store {
    pub db: Surreal<Db>,
    pub project: String,
}

impl Store {
    pub fn new(db: Surreal<Db>, project: impl Into<String>) -> Self {
        Self {
            db,
            project: project.into(),
        }
    }
}

/// Apply the atlas schema (idempotent — every statement is `IF NOT EXISTS`).
/// `embed_dim` is substituted into the HNSW `DIMENSION`.
pub async fn init_atlas_schema(db: &Surreal<Db>, embed_dim: usize) -> Result<()> {
    let schema = ATLAS_SCHEMA.replace("DIMENSION 384", &format!("DIMENSION {embed_dim}"));
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
                "atlas schema statement {i} failed: {e}\n  SQL: {}",
                stmt.chars().take(120).collect::<String>()
            );
            return Err(e.into());
        }
    }
    tracing::info!("atlas schema initialized");
    Ok(())
}

const ATLAS_SCHEMA: &str = r#"
-- One node in the corpus graph. `kind` ∈ document|concept|code_symbol|
-- external|file. `qualified` mirrors hifz's semantic path for code nodes
-- (projection target, E8). `cluster` = modularity cluster (E9, -1 = none).
DEFINE TABLE IF NOT EXISTS atlas_node SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project    ON atlas_node TYPE string;
DEFINE FIELD IF NOT EXISTS kind       ON atlas_node TYPE string;
DEFINE FIELD IF NOT EXISTS label      ON atlas_node TYPE string;
DEFINE FIELD IF NOT EXISTS qualified  ON atlas_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS path       ON atlas_node TYPE option<string>;
-- Source-agnostic provenance (forward-compatible: a future Slack/Jira/Notion
-- connector fills the same trio with a URL instead of a file path — no
-- migration, no downstream change). `source_kind` ∈ file|pdf|code|concept
-- now (later slack|jira|notion|web); `source_uri` = the one openable locator
-- (`file://…` now, `https://notion.so/…` later); `source_ref` = the human
-- breadcrumb shown as the citation text. NONE default → no migration runner.
DEFINE FIELD IF NOT EXISTS source_kind ON atlas_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS source_uri  ON atlas_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS source_ref  ON atlas_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS language   ON atlas_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS summary    ON atlas_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS embedding  ON atlas_node TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS cluster    ON atlas_node TYPE int DEFAULT -1;
DEFINE FIELD IF NOT EXISTS degree     ON atlas_node TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS created_at ON atlas_node TYPE string;
DEFINE INDEX IF NOT EXISTS atlas_node_kind ON TABLE atlas_node FIELDS project, kind;
DEFINE INDEX IF NOT EXISTS atlas_node_qual ON TABLE atlas_node FIELDS project, qualified;
DEFINE INDEX IF NOT EXISTS atlas_node_cluster ON TABLE atlas_node FIELDS project, cluster;

DEFINE ANALYZER IF NOT EXISTS atlas_analyzer TOKENIZERS blank, class FILTERS lowercase;
DEFINE INDEX IF NOT EXISTS atlas_node_label_ft ON TABLE atlas_node
  FIELDS label FULLTEXT ANALYZER atlas_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS atlas_node_summary_ft ON TABLE atlas_node
  FIELDS summary FULLTEXT ANALYZER atlas_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS atlas_node_vec ON TABLE atlas_node
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;

-- Typed edges. `resolution` ∈ resolved|external|ambiguous for code edges
-- (preserved from the hifz-core code-intel core, E8); concept/doc edges
-- carry the LLM/heuristic channel in `via`.
DEFINE TABLE IF NOT EXISTS atlas_edge SCHEMAFULL TYPE RELATION;
DEFINE FIELD IF NOT EXISTS relation   ON atlas_edge TYPE string;
DEFINE FIELD IF NOT EXISTS via        ON atlas_edge TYPE string DEFAULT 'atlas';
DEFINE FIELD IF NOT EXISTS score      ON atlas_edge TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS resolution ON atlas_edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS reason     ON atlas_edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS metadata   ON atlas_edge TYPE option<object>;
DEFINE FIELD IF NOT EXISTS created_at ON atlas_edge TYPE string;
DEFINE INDEX IF NOT EXISTS atlas_edge_relation ON TABLE atlas_edge FIELDS relation;

-- Text chunks for document/PDF nodes (hybrid-searchable like hifz chunks).
DEFINE TABLE IF NOT EXISTS atlas_chunk SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS node        ON atlas_chunk TYPE record<atlas_node>;
DEFINE FIELD IF NOT EXISTS project     ON atlas_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS chunk_index ON atlas_chunk TYPE int;
DEFINE FIELD IF NOT EXISTS content     ON atlas_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS embedding   ON atlas_chunk TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS created_at  ON atlas_chunk TYPE string;
DEFINE INDEX IF NOT EXISTS atlas_chunk_node ON TABLE atlas_chunk FIELDS node;
DEFINE INDEX IF NOT EXISTS atlas_chunk_project ON TABLE atlas_chunk FIELDS project;
DEFINE INDEX IF NOT EXISTS atlas_chunk_content_ft ON TABLE atlas_chunk
  FIELDS content FULLTEXT ANALYZER atlas_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS atlas_chunk_vec ON TABLE atlas_chunk
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::types::SurrealValue;

    #[tokio::test]
    async fn schema_inits_and_is_idempotent() {
        let db = kernel::db::connect_mem().await.unwrap();
        init_atlas_schema(&db, 384).await.unwrap();
        // Idempotent: second apply must also succeed.
        init_atlas_schema(&db, 384).await.unwrap();
        // Tables are usable.
        db.query(
            "CREATE atlas_node SET project='p', kind='document', label='README', \
             created_at='2026-01-01'",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        #[derive(Debug, SurrealValue)]
        struct C {
            c: Option<i64>,
        }
        let mut r = db
            .query("SELECT count() AS c FROM atlas_node GROUP ALL")
            .await
            .unwrap();
        let rows: Vec<C> = r.take(0).unwrap_or_default();
        assert_eq!(rows.into_iter().next().and_then(|x| x.c), Some(1));
    }

    #[tokio::test]
    async fn provenance_trio_fields_are_writable_and_readable() {
        let db = kernel::db::connect_mem().await.unwrap();
        init_atlas_schema(&db, 384).await.unwrap();
        db.query(
            "CREATE atlas_node SET project='p', kind='document', label='msa.pdf', \
             source_kind='pdf', source_uri='file:///abs/legal/msa.pdf', \
             source_ref='legal/msa.pdf', created_at='2026-01-01'",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        #[derive(Debug, SurrealValue)]
        struct Row {
            source_kind: Option<String>,
            source_uri: Option<String>,
            source_ref: Option<String>,
        }
        let mut r = db
            .query("SELECT source_kind, source_uri, source_ref FROM atlas_node")
            .await
            .unwrap();
        let row = r
            .take::<Vec<Row>>(0)
            .unwrap()
            .into_iter()
            .next()
            .expect("one node");
        assert_eq!(row.source_kind.as_deref(), Some("pdf"));
        assert_eq!(row.source_uri.as_deref(), Some("file:///abs/legal/msa.pdf"));
        assert_eq!(row.source_ref.as_deref(), Some("legal/msa.pdf"));
    }
}
