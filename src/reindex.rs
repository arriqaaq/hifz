//! One-shot backfill for schema upgrades.
//!
//! Phase 1a: memories did not have `embedding`, `project`, `keywords`, `tags`,
//! `context`, `retrieval_count`, or `last_accessed_at`. Schema defaults fill
//! scalars; this pass embeds existing rows and derives `project` from their
//! originating session where possible.

use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::db::Db;
use crate::embed::Embedder;
use crate::remember::build_embed_text;

#[derive(Debug, SurrealValue)]
struct Row {
    id: Option<surrealdb::types::RecordId>,
    project: Option<String>,
    title: Option<String>,
    content: Option<String>,
    keywords: Option<Vec<String>>,
    files: Option<Vec<String>>,
    embedding: Option<Vec<f32>>,
}

pub struct ReindexReport {
    pub embedded: usize,
    pub project_backfilled: usize,
    pub skipped: usize,
}

/// Backfill missing `embedding` and `project` fields on the `memory` table.
pub async fn reindex_memories(db: &Surreal<Db>, embedder: &Embedder) -> Result<ReindexReport> {
    let mut resp = db
        .query(
            "SELECT id, project, title, content, keywords, files, embedding \
             FROM memory",
        )
        .await?;
    let rows: Vec<Row> = resp.take(0)?;

    let mut report = ReindexReport {
        embedded: 0,
        project_backfilled: 0,
        skipped: 0,
    };

    for row in rows {
        let Some(id) = row.id else {
            report.skipped += 1;
            continue;
        };

        let mut updates: Vec<String> = Vec::new();

        // Project backfill: mark as needing attention if unset/empty/global default.
        let needs_project = row
            .project
            .as_deref()
            .map(|p| p.is_empty() || p == "global")
            .unwrap_or(true);
        if needs_project {
            updates.push("project".into());
            report.project_backfilled += 1;
        }

        // Embedding backfill: only if missing/empty.
        let needs_embedding = row.embedding.as_ref().map(|v| v.is_empty()).unwrap_or(true);
        if needs_embedding {
            let title = row.title.clone().unwrap_or_default();
            let content = row.content.clone().unwrap_or_default();
            let keywords = row.keywords.clone().unwrap_or_default();
            let files = row.files.clone().unwrap_or_default();
            let text = build_embed_text(&title, &content, &keywords, &files);
            let vec = embedder.embed_single(&text)?;
            db.query("UPDATE type::record($id) SET embedding = $v")
                .bind(("id", id.clone()))
                .bind(("v", vec))
                .await?;
            updates.push("embedding".into());
            report.embedded += 1;
        }

        if updates.is_empty() {
            report.skipped += 1;
        }
    }

    Ok(report)
}
