use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::embed::Embedder;
use crate::enrich;
use crate::models::Category;

/// Backwards-compatible thin wrapper for the deterministic insert path.
///
/// Production callers (the Hifz facade) should call `enrich::save_enriched`
/// directly with the LLM dependency and the full `RememberReq` field set.
/// This wrapper exists for benchmarks and any out-of-tree callers that still
/// pass positional args; it forces `enable_llm = false` so behavior is
/// deterministic and reproducible (no Ollama variance in benchmark numbers).
#[allow(clippy::too_many_arguments)] // positional-args compat wrapper by design; see doc above
pub async fn save(
    db: &Surreal<Db>,
    embedder: &Embedder,
    project: &str,
    category: &str,
    title: &str,
    content: &str,
    keywords: &[String],
    files: &[String],
    session_id: Option<&str>,
) -> Result<String> {
    enrich::save_enriched(
        db,
        embedder,
        None,
        false,
        project,
        title,
        content,
        Category::from_str(category),
        keywords.to_vec(),
        files.to_vec(),
        Vec::new(),
        None,
        None,
        None,
        session_id,
    )
    .await?;
    Ok(title.to_string())
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

// --- Memories search (lifted from web/api.rs::memories_search) ---

/// Search memories. With BM25 when a query is supplied; otherwise lists newest
/// first. Filters by project (with global fallback) and category. Returns the
/// legacy wire shape `{"memories": [...], "count": N}`.
pub async fn search(
    db: &surrealdb::Surreal<crate::db::Db>,
    params: crate::models::MemoriesReq,
) -> anyhow::Result<serde_json::Value> {
    let limit = params.limit.unwrap_or(50);
    let query = params.query.as_deref().unwrap_or("*");

    let mut conditions = vec!["is_latest = true".to_string()];
    if let Some(ref project) = params.project {
        conditions.push(format!(
            "(project = '{}' OR project = 'global')",
            project.replace('\'', "")
        ));
    }
    if let Some(ref category) = params.category {
        conditions.push(format!("category = '{}'", category.replace('\'', "")));
    }
    // Phase 5: time + open filters.
    if let Some(ref since) = params.since {
        conditions.push(format!("created_at >= '{}'", since.replace('\'', "")));
    }
    if params.open == Some(true) {
        conditions
            .push("id NOT IN (SELECT VALUE out FROM edge WHERE relation = 'closes')".to_string());
    }
    let where_clause = conditions.join(" AND ");

    let sql = if query.is_empty() || query == "*" {
        format!("SELECT * FROM memory WHERE {where_clause} ORDER BY created_at DESC LIMIT {limit}")
    } else {
        format!(
            "SELECT *, search::score(1) + search::score(2) AS _score FROM memory \
             WHERE {where_clause} AND (title @1@ $q OR content @2@ $q) \
             ORDER BY _score DESC LIMIT {limit}"
        )
    };

    let mut resp = if query != "*" && !query.is_empty() {
        db.query(&sql).bind(("q", query.to_string())).await?
    } else {
        db.query(&sql).await?
    };

    let memories: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let count = memories.len();
    Ok(serde_json::json!({"memories": memories, "count": count}))
}
