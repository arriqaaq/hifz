//! Plans — memory rows with `category = 'plan'`. Active plans carry the
//! `'active'` tag; completed/abandoned plans carry the corresponding tag.

use anyhow::Result;
use surrealdb::Surreal;

use crate::db::Db;
use crate::models::{PlanActivateReq, PlansListReq};

/// List plans, optionally filtered by project and status (which becomes a
/// `tags CONTAINS '<status>'` clause).
pub async fn list(db: &Surreal<Db>, params: PlansListReq) -> Result<serde_json::Value> {
    let project = params.project.as_deref().unwrap_or("");
    let status = params.status.as_deref();
    let limit = params.limit.unwrap_or(10);

    let mut conditions = vec!["category = 'plan' AND is_latest = true".to_string()];
    if !project.is_empty() {
        conditions.push("project = $project".to_string());
    }
    if let Some(s) = status {
        // Tag values are user-controlled but pass through a single-quote strip
        // to keep the SQL builder safe.
        conditions.push(format!("tags CONTAINS '{}'", s.replace('\'', "")));
    }

    let where_clause = format!(" WHERE {}", conditions.join(" AND "));
    let sql = format!("SELECT * FROM memory{where_clause} ORDER BY created_at DESC LIMIT {limit}");

    let mut q = db.query(&sql);
    if !project.is_empty() {
        q = q.bind(("project", project.to_string()));
    }
    let plans: Vec<serde_json::Value> = q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();
    Ok(serde_json::json!({"plans": plans}))
}

/// The currently active plan for a project (or globally if `project` is None).
pub async fn current(db: &Surreal<Db>, project: Option<&str>) -> Result<serde_json::Value> {
    let project_str = project.unwrap_or("");
    let mut conditions =
        vec!["category = 'plan' AND is_latest = true AND tags CONTAINS 'active'".to_string()];
    if !project_str.is_empty() {
        conditions.push("project = $project".to_string());
    }
    let where_clause = format!(" WHERE {}", conditions.join(" AND "));
    let sql = format!("SELECT * FROM memory{where_clause} LIMIT 1");

    let mut q = db.query(&sql);
    if !project_str.is_empty() {
        q = q.bind(("project", project_str.to_string()));
    }
    let plans: Vec<serde_json::Value> = q
        .await
        .ok()
        .and_then(|mut r| r.take(0).ok())
        .unwrap_or_default();
    Ok(plans.into_iter().next().unwrap_or(serde_json::Value::Null))
}

/// Mark a plan completed: replace the `active` tag with `completed`.
pub async fn complete(db: &Surreal<Db>, id: &str) -> Result<serde_json::Value> {
    transition_tag(db, id, "completed").await
}

/// Mark a plan abandoned: replace the `active` tag with `abandoned`.
pub async fn abandon(db: &Surreal<Db>, id: &str) -> Result<serde_json::Value> {
    transition_tag(db, id, "abandoned").await
}

/// Activate a plan: deactivate any currently-active plan for the project,
/// then add `active` to the requested plan_id.
pub async fn activate(db: &Surreal<Db>, body: PlanActivateReq) -> Result<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();

    // Deactivate any currently-active plan for this project.
    let _ = db
        .query(
            "UPDATE memory SET tags = array::filter(tags, |$t| $t != 'active') \
             WHERE category = 'plan' AND project = $project AND tags CONTAINS 'active'",
        )
        .bind(("project", body.project.clone()))
        .await;

    let Some(ref plan_id) = body.plan_id else {
        return Ok(serde_json::json!({"status": "no_active_plan"}));
    };

    let full_id = if plan_id.starts_with("memory:") {
        plan_id.clone()
    } else {
        format!("memory:{plan_id}")
    };

    let mut resp = db
        .query(
            "UPDATE type::record($id) SET \
             tags = array::distinct(array::concat(tags, ['active'])), \
             updated_at = $now \
             RETURN AFTER",
        )
        .bind(("id", full_id))
        .bind(("now", now))
        .await?;
    let plans: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    match plans.into_iter().next() {
        Some(p) => Ok(serde_json::json!({"status": "ok", "plan": p})),
        None => Ok(serde_json::json!({"status": "no_active_plan"})),
    }
}

async fn transition_tag(db: &Surreal<Db>, id: &str, new_tag: &str) -> Result<serde_json::Value> {
    let now = chrono::Utc::now().to_rfc3339();
    let full_id = if id.starts_with("memory:") {
        id.to_string()
    } else {
        format!("memory:{id}")
    };
    db.query(
        "UPDATE type::record($id) SET \
         tags = array::distinct(array::concat(array::filter(tags, |$t| $t != 'active'), [$tag])), \
         updated_at = $now",
    )
    .bind(("id", full_id))
    .bind(("now", now))
    .bind(("tag", new_tag.to_string()))
    .await?
    .check()?;
    Ok(serde_json::json!({"status": "ok"}))
}
