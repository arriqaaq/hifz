//! Plan tracking — first-class plan entities with lifecycle management.
//!
//! Plans capture `.claude/plans/*.md` files as structured records with:
//! - Status tracking (active, completed, abandoned)
//! - Linking to runs and commits
//! - Core memory sync for context injection

use anyhow::Result;
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, RecordIdKey, SurrealValue};

use crate::core_mem;
use crate::db::Db;

/// Extract the key portion of a RecordId as a string.
fn record_id_key_string(id: &RecordId) -> String {
    match &id.key {
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::Uuid(u) => u.to_string(),
        _ => format!("{:?}", id.key),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct PlanRow {
    pub id: Option<surrealdb::types::RecordId>,
    pub file_path: Option<String>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub status: Option<String>,
    pub project: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub files: Option<Vec<String>>,
    pub session_id: Option<surrealdb::types::RecordId>,
    pub commit_id: Option<surrealdb::types::RecordId>,
    pub created_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanUpsertRequest {
    pub file_path: String,
    pub title: String,
    pub content: String,
    pub project: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    pub session_id: Option<String>,
}

/// Upsert a plan by file_path (plan files can be edited multiple times).
pub async fn upsert(db: &Surreal<Db>, req: &PlanUpsertRequest) -> Result<PlanRow> {
    let now = chrono::Utc::now().to_rfc3339();

    let mut resp = db
        .query("SELECT * FROM plan WHERE file_path = $file_path LIMIT 1")
        .bind(("file_path", req.file_path.clone()))
        .await?;
    let existing: Vec<PlanRow> = resp.take(0).unwrap_or_default();

    if let Some(existing_plan) = existing.into_iter().next() {
        let id = existing_plan
            .id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan has no id"))?;
        let rid = format!("plan:{}", record_id_key_string(id));
        db.query(
            "UPDATE type::record($rid) SET 
                title = $title,
                content = $content,
                keywords = $keywords,
                files = $files",
        )
        .bind(("rid", rid))
        .bind(("title", req.title.clone()))
        .bind(("content", req.content.clone()))
        .bind(("keywords", req.keywords.clone()))
        .bind(("files", req.files.clone()))
        .await?;

        get_by_file_path(db, &req.file_path).await
    } else {
        let mut resp = db
            .query(
                "CREATE plan SET 
                    file_path = $file_path,
                    title = $title,
                    content = $content,
                    status = 'active',
                    project = $project,
                    keywords = $keywords,
                    files = $files,
                    session_id = $session_id,
                    created_at = $now",
            )
            .bind(("file_path", req.file_path.clone()))
            .bind(("title", req.title.clone()))
            .bind(("content", req.content.clone()))
            .bind(("project", req.project.clone()))
            .bind(("keywords", req.keywords.clone()))
            .bind(("files", req.files.clone()))
            .bind((
                "session_id",
                req.session_id.as_ref().map(|s| format!("session:{s}")),
            ))
            .bind(("now", now))
            .await?;

        let plans: Vec<PlanRow> = resp.take(0).unwrap_or_default();
        plans
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("failed to create plan"))
    }
}

/// Get a plan by its record ID.
pub async fn get(db: &Surreal<Db>, plan_id: &str) -> Result<Option<PlanRow>> {
    let rid = format!("plan:{plan_id}");
    let mut resp = db
        .query("SELECT * FROM type::record($rid)")
        .bind(("rid", rid))
        .await?;
    let plans: Vec<PlanRow> = resp.take(0).unwrap_or_default();
    Ok(plans.into_iter().next())
}

/// Get a plan by file path.
pub async fn get_by_file_path(db: &Surreal<Db>, file_path: &str) -> Result<PlanRow> {
    let mut resp = db
        .query("SELECT * FROM plan WHERE file_path = $file_path LIMIT 1")
        .bind(("file_path", file_path))
        .await?;
    let plans: Vec<PlanRow> = resp.take(0).unwrap_or_default();
    plans
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("plan not found"))
}

/// Get the most recent active plan for a project.
pub async fn get_active(db: &Surreal<Db>, project: &str) -> Result<Option<PlanRow>> {
    let mut resp = db
        .query(
            "SELECT * FROM plan 
             WHERE project = $project AND status = 'active' 
             ORDER BY created_at DESC 
             LIMIT 1",
        )
        .bind(("project", project))
        .await?;
    let plans: Vec<PlanRow> = resp.take(0).unwrap_or_default();
    Ok(plans.into_iter().next())
}

/// List plans for a project with optional status filter.
pub async fn list(
    db: &Surreal<Db>,
    project: &str,
    status: Option<&str>,
    limit: usize,
) -> Result<Vec<PlanRow>> {
    let sql = match status {
        Some("all") | None => {
            "SELECT * FROM plan WHERE project = $project ORDER BY created_at DESC LIMIT $limit"
        }
        Some(_) => {
            "SELECT * FROM plan WHERE project = $project AND status = $status ORDER BY created_at DESC LIMIT $limit"
        }
    };

    let mut resp = db
        .query(sql)
        .bind(("project", project))
        .bind(("status", status.unwrap_or("all")))
        .bind(("limit", limit))
        .await?;

    let plans: Vec<PlanRow> = resp.take(0).unwrap_or_default();
    Ok(plans)
}

/// Mark a plan as completed and clean up core memory.
pub async fn complete(db: &Surreal<Db>, plan_id: &str, commit_id: Option<&str>) -> Result<()> {
    let plan = get(db, plan_id).await?;
    if let Some(p) = plan {
        let now = chrono::Utc::now().to_rfc3339();
        let project = p.project.as_deref().unwrap_or("");

        // Remove from core memory goals
        let goal = format!("📋 {}", p.title.as_deref().unwrap_or(""));
        let _ = core_mem::edit(db, project, "goals", "remove", &goal).await;

        // Remove files from watchlist
        for file in p.files.as_ref().unwrap_or(&vec![]) {
            let _ = core_mem::edit(db, project, "watchlist", "remove", file).await;
        }

        // Mark plan as completed
        let rid = format!("plan:{plan_id}");
        let commit_ref = commit_id.map(|c| format!("commit:{c}"));
        db.query(
            "UPDATE type::record($rid) SET 
                status = 'completed', 
                completed_at = $now,
                commit_id = $cid",
        )
        .bind(("rid", rid))
        .bind(("now", now))
        .bind(("cid", commit_ref))
        .await?;
    }
    Ok(())
}

/// Mark a plan as abandoned.
pub async fn abandon(db: &Surreal<Db>, plan_id: &str) -> Result<()> {
    let plan = get(db, plan_id).await?;
    if let Some(p) = plan {
        let project = p.project.as_deref().unwrap_or("");

        // Remove from core memory goals
        let goal = format!("📋 {}", p.title.as_deref().unwrap_or(""));
        let _ = core_mem::edit(db, project, "goals", "remove", &goal).await;

        // Remove files from watchlist
        for file in p.files.as_ref().unwrap_or(&vec![]) {
            let _ = core_mem::edit(db, project, "watchlist", "remove", file).await;
        }

        let rid = format!("plan:{plan_id}");
        db.query("UPDATE type::record($rid) SET status = 'abandoned'")
            .bind(("rid", rid))
            .await?;
    }
    Ok(())
}

/// Activate a plan by syncing to core memory.
pub async fn activate(
    db: &Surreal<Db>,
    project: &str,
    plan_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<Option<PlanRow>> {
    let plan = match plan_id {
        Some(id) => get(db, id).await?,
        None => get_active(db, project).await?,
    };

    if let Some(p) = &plan {
        let proj = p.project.as_deref().unwrap_or(project);

        // Add plan title to core memory goals
        let goal = format!("📋 {}", p.title.as_deref().unwrap_or(""));
        let _ = core_mem::edit(db, proj, "goals", "add", &goal).await;

        // Add files to watchlist
        for file in p.files.as_ref().unwrap_or(&vec![]) {
            let _ = core_mem::edit(db, proj, "watchlist", "add", file).await;
        }

        // Motivated edges: recalled memories --motivated--> plan
        if let Some(sid) = session_id {
            if let Some(ref plan_rid) = p.id {
                if let Ok(Some(run_id)) = crate::run::find_open(db, sid).await {
                    if let Ok(recalled) = crate::run::get_recalled_ids(db, &run_id).await {
                        for mid in &recalled {
                            let _ = crate::link::upsert_edge(
                                db,
                                mid,
                                plan_rid,
                                "motivated",
                                "system",
                                0.7,
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }

    Ok(plan)
}

/// Link a commit to a plan.
pub async fn link_commit(db: &Surreal<Db>, commit_id: &str, plan_id: &str) -> Result<()> {
    let commit_rid = format!("commit:{commit_id}");
    let plan_rid = format!("plan:{plan_id}");
    db.query("UPDATE type::record($crid) SET plan_id = type::record($prid)")
        .bind(("crid", commit_rid))
        .bind(("prid", plan_rid))
        .await?;
    Ok(())
}
