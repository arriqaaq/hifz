//! maktab SurrealDB schema + connection.
//!
//! maktab tables live in the **same** SurrealKV instance as hifz (so the
//! corpus graph can later cross-link to hifz's grounded code/memory graph)
//! but are fully isolated: `maktab_*`, never touching hifz tables. Schema
//! init mirrors `kernel::db::init_schema` (strip `--` comments, split
//! by `;`, run each statement, fail loudly).

use anyhow::Result;
use kernel::db::Db;
use surrealdb::Surreal;
use surrealdb::types::RecordId;

/// Thin handle: the shared connection + the project the CLI/REST operate on.
/// `project` holds the project **slug** (the `project` table's record id key);
/// bind [`Store::pid`] into queries for the `record<project>` columns.
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

    /// The `record<project>` id for this store's project, i.e. `project:<slug>`.
    /// Bind this (not the bare slug string) into every query touching a
    /// `project` column on maktab_node / maktab_chunk / maktab_edge.
    pub fn pid(&self) -> RecordId {
        RecordId::new("project", self.project.as_str())
    }
}

/// Apply the maktab schema (idempotent — every statement is `IF NOT EXISTS`).
/// `embed_dim` is substituted into the HNSW `DIMENSION`.
pub async fn init_maktab_schema(db: &Surreal<Db>, embed_dim: usize) -> Result<()> {
    let schema = MAKTAB_SCHEMA.replace("DIMENSION 384", &format!("DIMENSION {embed_dim}"));
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
                "maktab schema statement {i} failed: {e}\n  SQL: {}",
                stmt.chars().take(120).collect::<String>()
            );
            return Err(e.into());
        }
    }
    tracing::info!("maktab schema initialized");
    Ok(())
}

const MAKTAB_SCHEMA: &str = r#"
-- Project: the first-class parent entity of the Maktab knowledge base. A user
-- creates a project by name, then ingests documents / indexes code into it;
-- every maktab_node/maktab_chunk/maktab_edge links here via `record<project>`.
-- The record id IS the slug (`project:my-kb`) so a reference is constructable
-- from a known slug without a lookup. Defined FIRST so the `record<project>`
-- field references below resolve at init. Scoped to Maktab only — the kernel
-- telemetry tables (session/observation/memory/…) keep their own string
-- `project` scoping and are NOT part of this entity.
DEFINE TABLE IF NOT EXISTS project SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name        ON project TYPE string;
DEFINE FIELD IF NOT EXISTS slug        ON project TYPE string;
DEFINE FIELD IF NOT EXISTS description ON project TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at  ON project TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at  ON project TYPE string;
DEFINE FIELD IF NOT EXISTS metadata    ON project TYPE option<object> FLEXIBLE;
DEFINE INDEX IF NOT EXISTS project_name_uniq ON TABLE project FIELDS name UNIQUE;
DEFINE INDEX IF NOT EXISTS project_slug_uniq ON TABLE project FIELDS slug UNIQUE;

-- One node in the corpus graph. `kind` ∈ document|concept|code_symbol|
-- external|file. `qualified` mirrors hifz's semantic path for code nodes
-- (projection target, E8). `cluster` = modularity cluster (E9, -1 = none).
DEFINE TABLE IF NOT EXISTS maktab_node SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS project    ON maktab_node TYPE record<project>;
DEFINE FIELD IF NOT EXISTS kind       ON maktab_node TYPE string;
DEFINE FIELD IF NOT EXISTS label      ON maktab_node TYPE string;
DEFINE FIELD IF NOT EXISTS qualified  ON maktab_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS path       ON maktab_node TYPE option<string>;
-- Source-agnostic provenance (forward-compatible: a future Slack/Jira/Notion
-- connector fills the same trio with a URL instead of a file path — no
-- migration, no downstream change). `source_kind` ∈ file|pdf|code|concept
-- now (later slack|jira|notion|web); `source_uri` = the one openable locator
-- (`file://…` now, `https://notion.so/…` later); `source_ref` = the human
-- breadcrumb shown as the citation text. NONE default → no migration runner.
DEFINE FIELD IF NOT EXISTS source_kind ON maktab_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS source_uri  ON maktab_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS source_ref  ON maktab_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS language   ON maktab_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS summary    ON maktab_node TYPE option<string>;
DEFINE FIELD IF NOT EXISTS embedding  ON maktab_node TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS cluster    ON maktab_node TYPE int DEFAULT -1;
DEFINE FIELD IF NOT EXISTS degree     ON maktab_node TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS created_at ON maktab_node TYPE string;
DEFINE INDEX IF NOT EXISTS maktab_node_kind ON TABLE maktab_node FIELDS project, kind;
DEFINE INDEX IF NOT EXISTS maktab_node_qual ON TABLE maktab_node FIELDS project, qualified;
DEFINE INDEX IF NOT EXISTS maktab_node_cluster ON TABLE maktab_node FIELDS project, cluster;

DEFINE ANALYZER IF NOT EXISTS maktab_analyzer TOKENIZERS blank, class FILTERS lowercase;
DEFINE INDEX IF NOT EXISTS maktab_node_label_ft ON TABLE maktab_node
  FIELDS label FULLTEXT ANALYZER maktab_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS maktab_node_summary_ft ON TABLE maktab_node
  FIELDS summary FULLTEXT ANALYZER maktab_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS maktab_node_vec ON TABLE maktab_node
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;

-- Typed edges. `resolution` ∈ resolved|external|ambiguous for code edges
-- (preserved from the hifz-core code-intel core, E8); concept/doc edges
-- carry the LLM/heuristic channel in `via`.
DEFINE TABLE IF NOT EXISTS maktab_edge SCHEMAFULL TYPE RELATION;
-- `project` is denormalized onto every edge (both endpoints belong to one
-- project by construction at every RELATE site). With the single-col
-- `maktab_edge_project` index, project-scoped edge reads use direct indexed
-- equality `WHERE project=$p` rather than an `in IN (SELECT …)` subquery
-- (forbidden by the `no_maktab_edge_in_subquery_anywhere_in_maktab` test). It
-- is a pure scoping key — never traversed; endpoints live in `in`/`out` —
-- so the invariant `edge.project == in.project == out.project` holds.
DEFINE FIELD IF NOT EXISTS project    ON maktab_edge TYPE record<project>;
DEFINE FIELD IF NOT EXISTS relation   ON maktab_edge TYPE string;
DEFINE FIELD IF NOT EXISTS via        ON maktab_edge TYPE string DEFAULT 'maktab';
DEFINE FIELD IF NOT EXISTS score      ON maktab_edge TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS resolution ON maktab_edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS reason     ON maktab_edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS metadata   ON maktab_edge TYPE option<object>;
DEFINE FIELD IF NOT EXISTS created_at ON maktab_edge TYPE string;
DEFINE INDEX IF NOT EXISTS maktab_edge_project  ON TABLE maktab_edge FIELDS project;
DEFINE INDEX IF NOT EXISTS maktab_edge_relation ON TABLE maktab_edge FIELDS relation;
DEFINE INDEX IF NOT EXISTS maktab_edge_via      ON TABLE maktab_edge FIELDS via;
-- Note: NO `maktab_edge_in`/`out` indexes — the planner doesn't use them for
-- `IN (subquery)`, and we no longer issue that query shape (verified hang
-- on 5879 edges); the `project` index does the actual work.

-- Text chunks for document/PDF nodes (hybrid-searchable like hifz chunks).
DEFINE TABLE IF NOT EXISTS maktab_chunk SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS node        ON maktab_chunk TYPE record<maktab_node>;
DEFINE FIELD IF NOT EXISTS project     ON maktab_chunk TYPE record<project>;
DEFINE FIELD IF NOT EXISTS chunk_index ON maktab_chunk TYPE int;
DEFINE FIELD IF NOT EXISTS content     ON maktab_chunk TYPE string;
DEFINE FIELD IF NOT EXISTS embedding   ON maktab_chunk TYPE option<array<float>>;
DEFINE FIELD IF NOT EXISTS created_at  ON maktab_chunk TYPE string;
DEFINE INDEX IF NOT EXISTS maktab_chunk_node ON TABLE maktab_chunk FIELDS node;
DEFINE INDEX IF NOT EXISTS maktab_chunk_project ON TABLE maktab_chunk FIELDS project;
DEFINE INDEX IF NOT EXISTS maktab_chunk_content_ft ON TABLE maktab_chunk
  FIELDS content FULLTEXT ANALYZER maktab_analyzer BM25 CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS maktab_chunk_vec ON TABLE maktab_chunk
  FIELDS embedding HNSW DIMENSION 384 DIST COSINE;
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::types::SurrealValue;

    #[tokio::test]
    async fn schema_inits_and_is_idempotent() {
        let db = kernel::db::connect_mem().await.unwrap();
        init_maktab_schema(&db, 384).await.unwrap();
        // Idempotent: second apply must also succeed.
        init_maktab_schema(&db, 384).await.unwrap();
        // Tables are usable.
        db.query(
            "CREATE maktab_node SET project='p', kind='document', label='README', \
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
            .query("SELECT count() AS c FROM maktab_node GROUP ALL")
            .await
            .unwrap();
        let rows: Vec<C> = r.take(0).unwrap_or_default();
        assert_eq!(rows.into_iter().next().and_then(|x| x.c), Some(1));
    }

    #[tokio::test]
    async fn provenance_trio_fields_are_writable_and_readable() {
        let db = kernel::db::connect_mem().await.unwrap();
        init_maktab_schema(&db, 384).await.unwrap();
        db.query(
            "CREATE maktab_node SET project='p', kind='document', label='msa.pdf', \
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
            .query("SELECT source_kind, source_uri, source_ref FROM maktab_node")
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

    /// Deterministic regression fence. The project-scoped edge queries
    /// (analyze/cluster/web) and via-filtered DELETEs (code/extract) all do
    /// `maktab_edge WHERE [via …] in IN (SELECT id FROM maktab_node WHERE
    /// project=$p)`. Without in/out/via indexes that full-scans every
    /// Deterministic schema fence. The fix to the project-scoped edge query
    /// hang (>18s on real data) is the denormalized `project` field on
    /// `maktab_edge` plus the single-col `maktab_edge_project` index — that's
    /// the only shape the planner reliably indexes in this surreal rev. If
    /// either is removed, this fails immediately (no silent regression).
    #[tokio::test]
    async fn maktab_edge_schema_has_project_field_and_index() {
        let db = kernel::db::connect_mem().await.unwrap();
        init_maktab_schema(&db, 384).await.unwrap();
        let mut r = db.query("INFO FOR TABLE maktab_edge").await.unwrap();
        let info: Option<serde_json::Value> = r.take(0).unwrap();
        let s = info.map(|v| v.to_string()).unwrap_or_default();
        for needed in [
            "project",             // field
            "maktab_edge_project", // index — THE one that makes the read fast
            "maktab_edge_relation",
            "maktab_edge_via",
        ] {
            assert!(
                s.contains(needed),
                "maktab_edge missing `{needed}` — INFO: {s}"
            );
        }
    }

    /// **Anti-pattern fence.** The bug class was a `WHERE in IN (SELECT … FROM
    /// maktab_node WHERE project=$p)` against `maktab_edge` — that shape hangs
    /// on real data regardless of `FIELDS in` indexes. This static-source
    /// check fails the build the instant anyone reintroduces it. (The
    /// previous behavioral test was too weak — 1500 in-mem edges aren't
    /// enough to discriminate the bug — so the *deterministic* guard is the
    /// source check.)
    #[test]
    fn no_maktab_edge_in_subquery_anywhere_in_maktab() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&crate_dir).unwrap() {
            let p = entry.unwrap().path();
            if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            // Strip the line each line that introduces the comparison; we're
            // matching the SQL pattern as a substring on a normalised view.
            let flat: String = src.split_whitespace().collect::<Vec<_>>().join(" ");
            // The exact failing shape: a SELECT/DELETE against maktab_edge
            // filtered by `in IN (SELECT ... FROM maktab_node`. We only flag
            // it inside an maktab_edge query (not in this very test, which
            // matches by literal *quoted* string in source — exclude this
            // file).
            if p.file_name().and_then(|s| s.to_str()) == Some("store.rs") {
                continue;
            }
            if flat.contains("maktab_edge WHERE in IN (SELECT")
                || flat.contains("maktab_edge WHERE via")
                    && flat.contains("in IN (SELECT VALUE id FROM maktab_node")
            {
                offenders.push(p.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "Re-introduced the unindexed `maktab_edge WHERE in IN (SELECT … maktab_node …)` \
             anti-pattern (hangs on real data) in: {offenders:?}. Use `WHERE project=$p` \
             against the denormalized field instead."
        );
    }

    /// Fast unit-test for the project-scoped edge query: a tiny fixture
    /// just large enough to catch a cross-project leak. **Correctness/
    /// isolation only — no perf assertion.** At-scale perf (the A/B vs the
    /// old `IN (subquery)` shape) lives in `benchmark/maktab_edge_scaling_bench.rs`;
    /// unit tests must stay sub-second. Together with the schema fence +
    /// anti-pattern source fence above, this is the durable per-PR guard.
    #[tokio::test]
    async fn project_scoped_edge_query_returns_only_target_project() {
        use surrealdb::types::RecordId;
        let db = kernel::db::connect_mem().await.unwrap();
        init_maktab_schema(&db, 384).await.unwrap();
        for (id, p) in [
            ("p1", "p"),
            ("p2", "p"),
            ("p3", "p"),
            ("n1", "n"),
            ("n2", "n"),
            ("n3", "n"),
            ("n4", "n"),
        ] {
            db.query(
                "CREATE type::record($id) SET project=$pp, kind='concept', \
                 label='x', cluster=-1, created_at='2026-01-01'",
            )
            .bind(("id", format!("maktab_node:{id}")))
            .bind(("pp", p.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
        }
        for (a, b, p) in [
            ("p1", "p2", "p"),
            ("p2", "p3", "p"),
            ("n1", "n2", "n"),
            ("n2", "n3", "n"),
            ("n3", "n4", "n"),
        ] {
            db.query(
                "RELATE $a->maktab_edge->$b SET project=$pp, relation='related', \
                 via='t', score=1.0, created_at='2026-01-01'",
            )
            .bind(("a", RecordId::new("maktab_node", a.to_string())))
            .bind(("b", RecordId::new("maktab_node", b.to_string())))
            .bind(("pp", p.to_string()))
            .await
            .unwrap();
        }
        #[derive(Debug, SurrealValue)]
        struct E {
            r#in: RecordId,
            out: RecordId,
        }
        let edges: Vec<E> = db
            .query("SELECT in, out FROM maktab_edge WHERE project=$pp")
            .bind(("pp", "p"))
            .await
            .unwrap()
            .take(0)
            .unwrap_or_default();
        assert_eq!(edges.len(), 2, "exactly p's 2 edges, no n leakage");
        for e in &edges {
            let ins = kernel::ids::rid_to_string(&e.r#in);
            assert!(
                ins.starts_with("maktab_node:p"),
                "leaked non-`p` edge: {ins}"
            );
        }
    }

    /// `/maktab/projects` integration: must list distinct maktab projects so
    /// the UI dropdown can union them with hifz sessions (the dst-not-
    /// appearing bug fix). Calls the production handler indirectly by
    /// seeding nodes and reading the same shape the handler uses.
    #[tokio::test]
    async fn distinct_maktab_projects_query_returns_each_once_sorted() {
        let db = kernel::db::connect_mem().await.unwrap();
        init_maktab_schema(&db, 384).await.unwrap();
        for (id, p) in [
            ("maktab_node:a", "alpha"),
            ("maktab_node:b", "alpha"), // duplicate
            ("maktab_node:c", "zeta"),
            ("maktab_node:d", "mid"),
        ] {
            db.query(
                "CREATE type::record($id) SET project=$pp, kind='concept', \
                 label='x', cluster=-1, created_at='2026-01-01'",
            )
            .bind(("id", id.to_string()))
            .bind(("pp", p.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
        }
        // Mirrors the handler in web.rs::projects exactly.
        let all: Vec<String> = db
            .query("SELECT VALUE project FROM maktab_node")
            .await
            .unwrap()
            .take(0)
            .unwrap_or_default();
        let projs: Vec<String> = all
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(
            projs,
            vec!["alpha".to_string(), "mid".into(), "zeta".into()]
        );
    }
}
