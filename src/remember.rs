use anyhow::Result;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;
use crate::embed::Embedder;
use crate::entities;
use crate::link;

/// Save an insight, decision, or pattern as a long-term memory.
///
/// Embeds richer text (title + content + keywords + files) so semantic search
/// works against saved memories, not just raw observations. Then runs Phase-3
/// deterministic link generation (KNN + keyword + file Jaccard) to populate
/// `memory_link` edges between the new memory and existing neighbours.
pub async fn save(
    db: &Surreal<Db>,
    embedder: &Embedder,
    project: &str,
    category: &str,
    title: &str,
    content: &str,
    keywords: &[String],
    files: &[String],
) -> Result<String> {
    let now = chrono::Utc::now().to_rfc3339();

    let embed_text = build_embed_text(title, content, keywords, files);
    let embedding = embedder.embed_single(&embed_text)?;

    #[derive(Debug, SurrealValue)]
    struct Created {
        id: Option<RecordId>,
    }

    let mut response = db
        .query(
            "CREATE memory SET \
             project = $project, \
             category = $category, \
             title = $title, \
             content = $content, \
             keywords = $keywords, \
             files = $files, \
             tags = [], \
             strength = 1.0, \
             retrieval_count = 0, \
             last_accessed_at = $now, \
             embedding = $embedding, \
             version = 1, \
             is_latest = true, \
             created_at = $now, \
             updated_at = $now \
             RETURN id",
        )
        .bind(("project", project.to_string()))
        .bind(("category", category.to_string()))
        .bind(("title", title.to_string()))
        .bind(("content", content.to_string()))
        .bind(("keywords", keywords.to_vec()))
        .bind(("files", files.to_vec()))
        .bind(("embedding", embedding.clone()))
        .bind(("now", now))
        .await?;
    response = response.check()?;
    let created: Vec<Created> = response.take(0).unwrap_or_default();

    // Fire deterministic link + entity generation if we recovered the new id.
    if let Some(new_id) = created.into_iter().next().and_then(|c| c.id) {
        if let Err(e) =
            link::generate_links(db, &new_id, project, &embedding, keywords, files).await
        {
            tracing::warn!("link generation failed for {new_id:?}: {e}");
        }

        // Entity extraction + via='entity' links.
        let body = format!("{title}\n{content}");
        for ent in entities::extract(files, keywords, &body) {
            let _ = entities::upsert(db, ent.kind, &ent.name, project).await;
        }
        if let Err(e) = link_by_shared_entities(db, &new_id, project, keywords, files).await {
            tracing::warn!("entity-link pass failed for {new_id:?}: {e}");
        }
    }

    Ok(title.to_string())
}

/// Link the new memory to existing memories it shares at least one entity
/// (file or keyword) with. `via='entity'` with score ∝ shared count.
async fn link_by_shared_entities(
    db: &Surreal<Db>,
    self_id: &RecordId,
    project: &str,
    keywords: &[String],
    files: &[String],
) -> Result<()> {
    if keywords.is_empty() && files.is_empty() {
        return Ok(());
    }

    // Find memories in the same project that share at least one file or keyword.
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
        keywords: Option<Vec<String>>,
        files: Option<Vec<String>>,
    }

    let mut resp = db
        .query(
            "SELECT id, keywords, files FROM memory \
             WHERE is_latest = true \
               AND id != $self \
               AND (project = $project OR project = 'global')",
        )
        .bind(("self", self_id.clone()))
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();

    let self_set: std::collections::HashSet<&str> = keywords
        .iter()
        .chain(files.iter())
        .map(String::as_str)
        .collect();

    for r in rows {
        let Some(other_id) = r.id else {
            continue;
        };
        let other_set: std::collections::HashSet<&str> = r
            .keywords
            .iter()
            .flatten()
            .chain(r.files.iter().flatten())
            .map(String::as_str)
            .collect();
        let shared = self_set.intersection(&other_set).count();
        if shared == 0 {
            continue;
        }
        let total = self_set.union(&other_set).count().max(1);
        let score = shared as f64 / total as f64; // Jaccard over keywords ∪ files
        link::upsert_link(db, self_id, &other_id, "entity", score).await?;
    }
    Ok(())
}

/// Delete a memory by ID.
pub async fn forget(db: &Surreal<Db>, memory_id: &str) -> Result<()> {
    db.query("DELETE type::record($id)")
        .bind(("id", memory_id.to_string()))
        .await?;
    Ok(())
}

/// Build the text that gets embedded for a memory.
/// Phase 1a: richer-text input — title + content + keywords + files.
pub fn build_embed_text(
    title: &str,
    content: &str,
    keywords: &[String],
    files: &[String],
) -> String {
    let mut s = String::with_capacity(title.len() + content.len() + 64);
    s.push_str(title);
    s.push('\n');
    s.push_str(content);
    if !keywords.is_empty() {
        s.push_str("\nkeywords: ");
        s.push_str(&keywords.join(", "));
    }
    if !files.is_empty() {
        s.push_str("\nfiles: ");
        s.push_str(&files.join(", "));
    }
    s
}
