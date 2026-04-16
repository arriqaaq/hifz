use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;

/// Save an insight, decision, or pattern as a long-term memory.
pub async fn save(
    db: &Surreal<Db>,
    mem_type: &str,
    title: &str,
    content: &str,
    concepts: &[String],
    files: &[String],
) -> Result<String> {
    let now = chrono::Utc::now().to_rfc3339();

    let response = db
        .query(
            "CREATE hifz SET \
             mem_type = $mem_type, \
             title = $title, \
             content = $content, \
             concepts = $concepts, \
             files = $files, \
             session_ids = [], \
             strength = 1.0, \
             version = 1, \
             is_latest = true, \
             created_at = $now, \
             updated_at = $now",
        )
        .bind(("mem_type", mem_type.to_string()))
        .bind(("title", title.to_string()))
        .bind(("content", content.to_string()))
        .bind(("concepts", concepts.to_vec()))
        .bind(("files", files.to_vec()))
        .bind(("now", now))
        .await?;
    response.check()?;

    Ok(title.to_string())
}

/// Delete a memory by ID.
pub async fn forget(db: &Surreal<Db>, memory_id: &str) -> Result<()> {
    db.query("DELETE type::record($id)")
        .bind(("id", memory_id.to_string()))
        .await?;
    Ok(())
}
