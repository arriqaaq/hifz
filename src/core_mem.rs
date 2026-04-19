//! Core memory — MemGPT-style per-project always-on block.
//!
//! One row per project, always prepended to injected context so identity,
//! active goals, invariants, and watched items never drift out on compaction.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::types::SurrealValue;

use crate::db::Db;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct CoreRow {
    pub project: Option<String>,
    pub identity: Option<String>,
    pub goals: Option<Vec<String>>,
    pub invariants: Option<Vec<String>>,
    pub watchlist: Option<Vec<String>>,
    pub updated_at: Option<String>,
}

/// Fetch the core block for a project. Returns an empty row if none exists.
pub async fn get(db: &Surreal<Db>, project: &str) -> Result<CoreRow> {
    let mut resp = db
        .query("SELECT * FROM hifz_core WHERE project = $project LIMIT 1")
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<CoreRow> = resp.take(0).unwrap_or_default();
    Ok(rows.into_iter().next().unwrap_or(CoreRow {
        project: Some(project.to_string()),
        identity: None,
        goals: Some(vec![]),
        invariants: Some(vec![]),
        watchlist: Some(vec![]),
        updated_at: None,
    }))
}

/// Apply a patch to the core block. `field` must be one of `identity`, `goals`,
/// `invariants`, `watchlist`. `op` is `set` (replace), `add`, or `remove`.
/// For `identity`, only `set` makes sense; for the others, `add` appends and
/// `remove` drops a matching string.
pub async fn edit(
    db: &Surreal<Db>,
    project: &str,
    field: &str,
    op: &str,
    value: &str,
) -> Result<CoreRow> {
    let now = chrono::Utc::now().to_rfc3339();

    // Upsert-or-create the singleton row.
    db.query(
        "IF (SELECT count() FROM hifz_core WHERE project = $project GROUP ALL)[0].count == 0 \
         THEN CREATE hifz_core SET project = $project, updated_at = $now END",
    )
    .bind(("project", project.to_string()))
    .bind(("now", now.clone()))
    .await?;

    let sql = match (field, op) {
        ("identity", _) => {
            "UPDATE hifz_core SET identity = $value, updated_at = $now WHERE project = $project"
        }
        ("goals", "add") => {
            "UPDATE hifz_core SET goals = array::distinct(array::concat(goals, [$value])), updated_at = $now WHERE project = $project"
        }
        ("goals", "remove") => {
            "UPDATE hifz_core SET goals = array::difference(goals, [$value]), updated_at = $now WHERE project = $project"
        }
        ("invariants", "add") => {
            "UPDATE hifz_core SET invariants = array::distinct(array::concat(invariants, [$value])), updated_at = $now WHERE project = $project"
        }
        ("invariants", "remove") => {
            "UPDATE hifz_core SET invariants = array::difference(invariants, [$value]), updated_at = $now WHERE project = $project"
        }
        ("watchlist", "add") => {
            "UPDATE hifz_core SET watchlist = array::distinct(array::concat(watchlist, [$value])), updated_at = $now WHERE project = $project"
        }
        ("watchlist", "remove") => {
            "UPDATE hifz_core SET watchlist = array::difference(watchlist, [$value]), updated_at = $now WHERE project = $project"
        }
        _ => {
            return Err(anyhow::anyhow!(
                "unsupported (field, op): ({field}, {op}) — expected identity/set or goals|invariants|watchlist / add|remove"
            ));
        }
    };

    db.query(sql)
        .bind(("project", project.to_string()))
        .bind(("value", value.to_string()))
        .bind(("now", now))
        .await?
        .check()?;

    get(db, project).await
}

/// Render the core block as markdown for injection into context. Empty string if nothing set.
pub fn render(row: &CoreRow) -> String {
    let mut out = String::new();
    let identity = row.identity.as_deref().unwrap_or("").trim();
    let goals = row.goals.clone().unwrap_or_default();
    let invariants = row.invariants.clone().unwrap_or_default();
    let watchlist = row.watchlist.clone().unwrap_or_default();

    if identity.is_empty() && goals.is_empty() && invariants.is_empty() && watchlist.is_empty() {
        return out;
    }

    out.push_str("# Core\n\n");
    if !identity.is_empty() {
        out.push_str("**Identity:** ");
        out.push_str(identity);
        out.push('\n');
    }
    if !goals.is_empty() {
        out.push_str("**Goals:**\n");
        for g in &goals {
            out.push_str("- ");
            out.push_str(g);
            out.push('\n');
        }
    }
    if !invariants.is_empty() {
        out.push_str("**Invariants:**\n");
        for i in &invariants {
            out.push_str("- ");
            out.push_str(i);
            out.push('\n');
        }
    }
    if !watchlist.is_empty() {
        out.push_str("**Watchlist:**\n");
        for w in &watchlist {
            out.push_str("- ");
            out.push_str(w);
            out.push('\n');
        }
    }
    out.push('\n');
    out
}
