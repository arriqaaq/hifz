//! Project: the first-class parent entity of the Maktab knowledge base.
//!
//! A user creates a project by name; everything they ingest (documents,
//! indexed code, future links) is scoped to it via `record<project>` on
//! `maktab_node` / `maktab_chunk` / `maktab_edge`. The record id **is the slug**
//! (`project:my-kb`), so a reference is constructable from a known slug with
//! no lookup ([`rid`]). Creation is **explicit** — nothing auto-creates a
//! project; build/ingest handlers reject an unknown slug (see `web::require`).
//!
//! Scoped to Maktab only: the kernel telemetry tables (session/observation/
//! memory/…) keep their own cwd-derived string `project` and are NOT part of
//! this entity.

use anyhow::{Result, bail};
use kernel::db::Db;
use kernel::ids::rid_to_string;
use serde_json::{Value, json};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

/// Normalize a human name into a slug usable as the `project` record-id key:
/// lowercase, runs of non-alphanumerics collapse to a single `-`, trimmed.
/// `"My KB!"` → `"my-kb"`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
            pending_dash = false;
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The `record<project>` id for a slug, i.e. `project:<slug>`.
pub fn rid(slug: &str) -> RecordId {
    RecordId::new("project", slug)
}

/// Does a project with this slug exist? (Reads never auto-create.)
pub async fn exists(db: &Surreal<Db>, slug: &str) -> Result<bool> {
    #[derive(SurrealValue)]
    struct R {
        #[allow(dead_code)]
        id: RecordId,
    }
    let mut r = db
        .query("SELECT id FROM project WHERE slug=$s")
        .bind(("s", slug.to_string()))
        .await?;
    Ok(!r.take::<Vec<R>>(0).unwrap_or_default().is_empty())
}

/// Create a project from a human name. Slug + name are UNIQUE — a collision is
/// an error (the UI should surface it, not silently reuse).
pub async fn create(db: &Surreal<Db>, name: &str, description: Option<String>) -> Result<Value> {
    let name = name.trim();
    if name.is_empty() {
        bail!("project name is empty");
    }
    let slug = slugify(name);
    if slug.is_empty() {
        bail!("project name has no slug-able characters");
    }
    if exists(db, &slug).await? {
        bail!("project '{slug}' already exists");
    }
    let now = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE type::record($id) SET name=$n, slug=$s, description=$d, \
         created_at=$t, updated_at=$t",
    )
    .bind(("id", format!("project:{slug}")))
    .bind(("n", name.to_string()))
    .bind(("s", slug.clone()))
    .bind(("d", description))
    .bind(("t", now))
    .await?
    .check()?;
    get(db, &slug)
        .await?
        .ok_or_else(|| anyhow::anyhow!("project created but not found"))
}

/// Fetch one project's row by slug.
pub async fn get(db: &Surreal<Db>, slug: &str) -> Result<Option<Value>> {
    let mut r = db
        .query(
            "SELECT id, name, slug, description, created_at, updated_at \
             FROM project WHERE slug=$s",
        )
        .bind(("s", slug.to_string()))
        .await?;
    Ok(r.take::<Vec<Value>>(0)
        .unwrap_or_default()
        .into_iter()
        .next())
}

/// Update a project's description.
pub async fn patch(db: &Surreal<Db>, slug: &str, description: Option<String>) -> Result<Value> {
    if !exists(db, slug).await? {
        bail!("project '{slug}' does not exist");
    }
    let now = chrono::Utc::now().to_rfc3339();
    db.query("UPDATE type::record($id) SET description=$d, updated_at=$t")
        .bind(("id", format!("project:{slug}")))
        .bind(("d", description))
        .bind(("t", now))
        .await?
        .check()?;
    get(db, slug)
        .await?
        .ok_or_else(|| anyhow::anyhow!("project not found after patch"))
}

/// Delete a project and cascade its whole corpus. Order: edges (a RELATION
/// table whose endpoints are nodes) → chunks → nodes → the project row.
pub async fn delete(db: &Surreal<Db>, slug: &str) -> Result<()> {
    let pid = rid(slug);
    for tbl in ["maktab_edge", "maktab_chunk", "maktab_node"] {
        db.query(format!("DELETE {tbl} WHERE project=$p"))
            .bind(("p", pid.clone()))
            .await?
            .check()?;
    }
    db.query("DELETE type::record($id)")
        .bind(("id", format!("project:{slug}")))
        .await?
        .check()?;
    Ok(())
}

/// All projects with per-project corpus counts (documents / code / total
/// nodes / edges), joined in Rust on the project record id.
pub async fn list_with_counts(db: &Surreal<Db>) -> Result<Value> {
    #[derive(SurrealValue)]
    struct ProjRow {
        id: RecordId,
        name: Option<String>,
        slug: Option<String>,
        description: Option<String>,
        created_at: Option<String>,
        updated_at: Option<String>,
    }
    #[derive(SurrealValue)]
    struct NodeGroup {
        project: Option<RecordId>,
        kind: Option<String>,
        c: Option<i64>,
    }
    #[derive(SurrealValue)]
    struct EdgeGroup {
        project: Option<RecordId>,
        c: Option<i64>,
    }

    let mut pr = db
        .query(
            "SELECT id, name, slug, description, created_at, updated_at \
             FROM project ORDER BY created_at",
        )
        .await?;
    let projects: Vec<ProjRow> = pr.take(0).unwrap_or_default();

    let mut nq = db
        .query("SELECT project, kind, count() AS c FROM maktab_node GROUP BY project, kind")
        .await?;
    let node_counts: Vec<NodeGroup> = nq.take(0).unwrap_or_default();
    let mut eq = db
        .query("SELECT project, count() AS c FROM maktab_edge GROUP BY project")
        .await?;
    let edge_counts: Vec<EdgeGroup> = eq.take(0).unwrap_or_default();

    let out: Vec<Value> = projects
        .iter()
        .map(|p| {
            let pid = rid_to_string(&p.id);
            let (mut documents, mut code, mut nodes) = (0i64, 0i64, 0i64);
            for g in &node_counts {
                if g.project.as_ref().map(rid_to_string).as_deref() == Some(pid.as_str()) {
                    let c = g.c.unwrap_or(0);
                    nodes += c;
                    match g.kind.as_deref() {
                        Some("document") => documents += c,
                        Some("code_symbol") => code += c,
                        _ => {}
                    }
                }
            }
            let edges = edge_counts
                .iter()
                .find(|g| g.project.as_ref().map(rid_to_string).as_deref() == Some(pid.as_str()))
                .and_then(|g| g.c)
                .unwrap_or(0);
            json!({
                "id": pid,
                "name": p.name,
                "slug": p.slug,
                "description": p.description,
                "created_at": p.created_at,
                "updated_at": p.updated_at,
                "counts": { "documents": documents, "code": code, "nodes": nodes, "edges": edges },
            })
        })
        .collect();
    Ok(json!({ "projects": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("My KB!"), "my-kb");
        assert_eq!(slugify("  Hello   World  "), "hello-world");
        assert_eq!(slugify("course:42"), "course-42");
        assert_eq!(slugify("already-slug"), "already-slug");
        assert_eq!(slugify("UPPER_case"), "upper-case");
        assert_eq!(slugify("!!!"), "");
    }

    #[tokio::test]
    async fn create_list_delete_roundtrip() {
        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_maktab_schema(&db, 384).await.unwrap();

        assert!(!exists(&db, "my-kb").await.unwrap());
        let p = create(&db, "My KB", Some("notes".into())).await.unwrap();
        assert_eq!(p.get("slug").and_then(|v| v.as_str()), Some("my-kb"));
        assert!(exists(&db, "my-kb").await.unwrap());

        // Duplicate name/slug is rejected.
        assert!(create(&db, "My KB", None).await.is_err());

        let listed = list_with_counts(&db).await.unwrap();
        let arr = listed.get("projects").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0]
                .get("counts")
                .and_then(|c| c.get("nodes"))
                .and_then(|v| v.as_i64()),
            Some(0)
        );

        delete(&db, "my-kb").await.unwrap();
        assert!(!exists(&db, "my-kb").await.unwrap());
    }
}
