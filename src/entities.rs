//! Deterministic entity extraction from observations and memories.
//!
//! All four extractors are regex/string-based — no LLM. Entities are upserted
//! into the `entity` table and flow into `edge` table with
//! `relation='mentions', via='entity'` via `link.rs`.

use std::collections::HashSet;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

/// Kinds we extract. Keep the list small; new kinds should have a clear,
/// testable extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    File,
    Symbol,
    Concept,
    Error,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Symbol => "symbol",
            Kind::Concept => "concept",
            Kind::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Extracted {
    pub kind: Kind,
    pub name: String,
}

/// Pull entities from the various raw fields associated with an observation
/// or memory: the existing `files` and `keywords` arrays, plus regex-extracted
/// symbols and error codes from a free-text body.
pub fn extract(files: &[String], keywords: &[String], body: &str) -> Vec<Extracted> {
    let mut out: HashSet<Extracted> = HashSet::new();

    for f in files {
        let name = f.trim();
        if !name.is_empty() {
            out.insert(Extracted {
                kind: Kind::File,
                name: name.to_string(),
            });
        }
    }
    for c in keywords {
        let name = c.trim();
        if !name.is_empty() {
            out.insert(Extracted {
                kind: Kind::Concept,
                name: name.to_lowercase(),
            });
        }
    }

    for m in SYMBOL_RE.captures_iter(body) {
        if let Some(cap) = m.get(1) {
            let name = cap.as_str().trim();
            if !is_noisy_symbol(name) {
                out.insert(Extracted {
                    kind: Kind::Symbol,
                    name: name.to_string(),
                });
            }
        }
    }

    for m in ERROR_RE.find_iter(body) {
        let name = m.as_str().trim();
        out.insert(Extracted {
            kind: Kind::Error,
            name: name.to_string(),
        });
    }

    out.into_iter().collect()
}

// Matches common Rust/Python/JS/TS symbol definitions:
//   fn name(, def name(, class Name, impl Name
// Capture group 1 is the identifier.
static SYMBOL_RE_SRC: &str = r"\b(?:fn|def|class|impl)\s+([A-Za-z_][A-Za-z0-9_]*)";
// Matches E followed by 3+ digits (Rust-ish error codes) or HTTP statuses 4xx/5xx.
static ERROR_RE_SRC: &str = r"\bE\d{3,}\b|\b[45]\d{2}\b";

static SYMBOL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(SYMBOL_RE_SRC).expect("symbol regex"));
static ERROR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ERROR_RE_SRC).expect("error regex"));

// Drop common-word false positives that sneak past the symbol regex. Keep
// the list short — overfitting here loses signal.
fn is_noisy_symbol(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "" | "the" | "a" | "an" | "is" | "of" | "to" | "and" | "or"
    )
}

/// Upsert an entity row, bumping `count` and `last_seen` on hit.
pub async fn upsert(
    db: &Surreal<Db>,
    kind: Kind,
    name: &str,
    project: &str,
) -> Result<Option<RecordId>> {
    let now = chrono::Utc::now().to_rfc3339();

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }

    let mut resp = db
        .query(
            "SELECT id FROM entity \
             WHERE kind = $kind AND name = $name AND project = $project \
             LIMIT 1",
        )
        .bind(("kind", kind.as_str().to_string()))
        .bind(("name", name.to_string()))
        .bind(("project", project.to_string()))
        .await?;
    let existing: Vec<Row> = resp.take(0).unwrap_or_default();

    if let Some(row) = existing.into_iter().next() {
        if let Some(id) = row.id {
            db.query("UPDATE type::record($id) SET count += 1, last_seen = $now")
                .bind(("id", id.clone()))
                .bind(("now", now))
                .await?
                .check()?;
            return Ok(Some(id));
        }
    }

    let mut create = db
        .query(
            "CREATE entity SET kind = $kind, name = $name, project = $project, \
             first_seen = $now, last_seen = $now, count = 1 RETURN id",
        )
        .bind(("kind", kind.as_str().to_string()))
        .bind(("name", name.to_string()))
        .bind(("project", project.to_string()))
        .bind(("now", now))
        .await?;
    create = create.check()?;
    let created: Vec<Row> = create.take(0).unwrap_or_default();
    Ok(created.into_iter().next().and_then(|r| r.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_files_and_keywords() {
        let files = vec!["src/search.rs".to_string()];
        let keywords = vec!["ranking".to_string()];
        let out = extract(&files, &keywords, "");
        assert!(
            out.iter()
                .any(|e| e.kind == Kind::File && e.name == "src/search.rs")
        );
        assert!(
            out.iter()
                .any(|e| e.kind == Kind::Concept && e.name == "ranking")
        );
    }

    #[test]
    fn extracts_rust_symbols() {
        let out = extract(&[], &[], "pub fn hello() -> u32 { 1 }");
        assert!(
            out.iter()
                .any(|e| e.kind == Kind::Symbol && e.name == "hello")
        );
    }

    #[test]
    fn extracts_python_class() {
        let out = extract(&[], &[], "class FooBar:\n    pass");
        assert!(
            out.iter()
                .any(|e| e.kind == Kind::Symbol && e.name == "FooBar")
        );
    }

    #[test]
    fn extracts_error_codes() {
        let out = extract(&[], &[], "rustc threw E0277 and the server returned 500");
        assert!(
            out.iter()
                .any(|e| e.kind == Kind::Error && e.name == "E0277")
        );
        assert!(out.iter().any(|e| e.kind == Kind::Error && e.name == "500"));
    }
}
