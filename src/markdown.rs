//! Markdown round-trip for memories.
//!
//! Renders a memory as a frontmatter-rich markdown file (the form Obsidian
//! and similar tools natively understand) and parses an edited markdown
//! file back into the fields needed to update the row.
//!
//! Frontmatter format is YAML-ish (key: value lines, list values as
//! `[a, b, c]` or block-style `- a` lines). We use a hand-rolled parser to
//! avoid pulling in a YAML dep — the schema is small and stable.
//!
//! Round-trip contract:
//!   1. `GET /memories/{id}/markdown` → frontmatter has every field hifz
//!      needs to reconstruct the row, body has `content_long` (or `content`
//!      if no long form).
//!   2. User edits the file (in Obsidian, $EDITOR, etc.).
//!   3. `PUT /memories/{id}/markdown` parses frontmatter + body, calls into
//!      the `enrich::save_enriched` pipeline as a NEW memory with
//!      `supersedes_memory_id = old_id` set. The old row gets `is_latest =
//!      false` automatically; a `supersedes` edge points new→old.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::db::Db;

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct MarkdownDoc {
    pub frontmatter: Frontmatter,
    pub body: String,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Frontmatter {
    pub id: Option<String>,
    pub project: Option<String>,
    pub category: Option<String>,
    pub title: Option<String>,
    pub keywords: Vec<String>,
    pub files: Vec<String>,
    pub tags: Vec<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub version: Option<i64>,
}

// ---------------------------------------------------------------------------
// GET — render
// ---------------------------------------------------------------------------

/// Fetch a memory by id and render as frontmatter+markdown.
pub async fn render(db: &Surreal<Db>, memory_id: &str) -> Result<String> {
    let normalized = if memory_id.starts_with("memory:") {
        memory_id.to_string()
    } else {
        format!("memory:{memory_id}")
    };

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
        project: Option<String>,
        category: Option<String>,
        title: Option<String>,
        content: Option<String>,
        content_long: Option<String>,
        keywords: Option<Vec<String>>,
        files: Option<Vec<String>>,
        tags: Option<Vec<String>>,
        created_at: Option<String>,
        updated_at: Option<String>,
        version: Option<i64>,
    }

    let mut resp = db
        .query(
            "SELECT id, project, category, title, content, content_long, \
             keywords, files, tags, created_at, updated_at, version \
             FROM type::record($id)",
        )
        .bind(("id", normalized))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();
    let row = rows
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("memory not found: {memory_id}"))?;

    let id_str = row.id.map(|r| format!("{r:?}")).unwrap_or_default();
    let category = row.category.unwrap_or_else(|| "note".to_string());
    let body = row
        .content_long
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| row.content.clone().unwrap_or_default());

    let mut s = String::with_capacity(body.len() + 512);
    s.push_str("---\n");
    s.push_str(&format!("id: {id_str}\n"));
    if let Some(p) = row.project {
        s.push_str(&format!("project: {p}\n"));
    }
    s.push_str(&format!("category: {category}\n"));
    if let Some(t) = row.title {
        s.push_str(&format!("title: {}\n", yaml_escape(&t)));
    }
    s.push_str(&format!(
        "keywords: {}\n",
        yaml_inline_list(&row.keywords.unwrap_or_default())
    ));
    s.push_str(&format!(
        "files: {}\n",
        yaml_inline_list(&row.files.unwrap_or_default())
    ));
    s.push_str(&format!(
        "tags: {}\n",
        yaml_inline_list(&row.tags.unwrap_or_default())
    ));
    if let Some(c) = row.created_at {
        s.push_str(&format!("created_at: {c}\n"));
    }
    if let Some(u) = row.updated_at {
        s.push_str(&format!("updated_at: {u}\n"));
    }
    if let Some(v) = row.version {
        s.push_str(&format!("version: {v}\n"));
    }
    s.push_str("---\n\n");
    s.push_str(&body);
    if !s.ends_with('\n') {
        s.push('\n');
    }
    Ok(s)
}

// ---------------------------------------------------------------------------
// PUT — parse
// ---------------------------------------------------------------------------

/// Parse a frontmatter+body markdown blob. Returns the parsed
/// `MarkdownDoc`. Does NOT touch the DB — callers feed the result into
/// `enrich::save_enriched` to write the new version.
pub fn parse(input: &str) -> Result<MarkdownDoc> {
    let trimmed = input.trim_start_matches('\u{FEFF}');
    let lines: Vec<&str> = trimmed.lines().collect();

    // Frontmatter is bracketed by `---` lines. If absent, treat the whole
    // input as the body with empty frontmatter.
    if lines.first().map(|l| l.trim()) != Some("---") {
        return Ok(MarkdownDoc {
            frontmatter: Frontmatter::default(),
            body: trimmed.to_string(),
        });
    }

    let mut end = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end = Some(i);
            break;
        }
    }
    let end = end.context("unterminated frontmatter")?;

    let frontmatter_body: HashMap<String, String> = lines[1..end]
        .iter()
        .filter_map(|line| parse_frontmatter_line(line))
        .collect();

    let body = lines[end + 1..].join("\n").trim_start().to_string();

    let fm = Frontmatter {
        id: frontmatter_body.get("id").cloned(),
        project: frontmatter_body.get("project").cloned(),
        category: frontmatter_body.get("category").cloned(),
        title: frontmatter_body.get("title").map(|s| yaml_unescape(s)),
        keywords: frontmatter_body
            .get("keywords")
            .map(|s| parse_inline_list(s))
            .unwrap_or_default(),
        files: frontmatter_body
            .get("files")
            .map(|s| parse_inline_list(s))
            .unwrap_or_default(),
        tags: frontmatter_body
            .get("tags")
            .map(|s| parse_inline_list(s))
            .unwrap_or_default(),
        created_at: frontmatter_body.get("created_at").cloned(),
        updated_at: frontmatter_body.get("updated_at").cloned(),
        version: frontmatter_body
            .get("version")
            .and_then(|s| s.trim().parse().ok()),
    };

    Ok(MarkdownDoc {
        frontmatter: fm,
        body,
    })
}

fn parse_frontmatter_line(line: &str) -> Option<(String, String)> {
    let line = line.trim_end();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let idx = line.find(':')?;
    let key = line[..idx].trim().to_string();
    let value = line[idx + 1..].trim().to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

// ---------------------------------------------------------------------------
// YAML-ish helpers
// ---------------------------------------------------------------------------

fn yaml_inline_list(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let parts: Vec<String> = items.iter().map(|s| yaml_escape(s)).collect();
    format!("[{}]", parts.join(", "))
}

fn parse_inline_list(s: &str) -> Vec<String> {
    let s = s.trim();
    if s == "[]" || s.is_empty() {
        return Vec::new();
    }
    let inner = s.trim_start_matches('[').trim_end_matches(']');
    split_top_level(inner)
        .into_iter()
        .map(|p| yaml_unescape(p.trim()))
        .filter(|p| !p.is_empty())
        .collect()
}

/// Split on commas that are NOT inside a double-quoted segment, so a
/// quoted element containing `,` (e.g. `"d, e"`) survives a round-trip
/// through `yaml_escape` / `parse_inline_list`.
fn split_top_level(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for c in inner.chars() {
        if escaped {
            buf.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_quotes => {
                buf.push(c);
                escaped = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                buf.push(c);
            }
            ',' if !in_quotes => {
                out.push(std::mem::take(&mut buf));
            }
            _ => buf.push(c),
        }
    }
    out.push(buf);
    out
}

/// Escape a value for YAML inline form. Quotes if it contains commas,
/// brackets, or starts with a number/special char.
fn yaml_escape(s: &str) -> String {
    let needs_quote = s.is_empty()
        || s.contains(',')
        || s.contains('[')
        || s.contains(']')
        || s.contains(':')
        || s.contains('"')
        || s.contains('\'')
        || s.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '!' | '&' | '*'));
    if needs_quote {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

fn yaml_unescape(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        let inner = &s[1..s.len() - 1];
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple() {
        let input = "---\nid: memory:abc\ncategory: lesson\ntitle: Hello world\nkeywords: [auth, jwt]\nfiles: [src/auth.rs]\ntags: []\n---\n\nThe body of the note.\n";
        let doc = parse(input).unwrap();
        assert_eq!(doc.frontmatter.id.as_deref(), Some("memory:abc"));
        assert_eq!(doc.frontmatter.category.as_deref(), Some("lesson"));
        assert_eq!(doc.frontmatter.title.as_deref(), Some("Hello world"));
        assert_eq!(
            doc.frontmatter.keywords,
            vec!["auth".to_string(), "jwt".to_string()]
        );
        assert_eq!(doc.frontmatter.files, vec!["src/auth.rs".to_string()]);
        assert!(doc.frontmatter.tags.is_empty());
        assert_eq!(doc.body.trim(), "The body of the note.");
    }

    #[test]
    fn parse_handles_quoted_title_with_colon() {
        let input = "---\ntitle: \"Refactor: extract auth helper\"\ncategory: plan\nkeywords: []\nfiles: []\ntags: []\n---\nbody\n";
        let doc = parse(input).unwrap();
        assert_eq!(
            doc.frontmatter.title.as_deref(),
            Some("Refactor: extract auth helper")
        );
    }

    #[test]
    fn parse_no_frontmatter_treats_whole_as_body() {
        let input = "no frontmatter here\njust text";
        let doc = parse(input).unwrap();
        assert!(doc.frontmatter.title.is_none());
        assert_eq!(doc.body, input);
    }

    #[test]
    fn parse_unterminated_frontmatter_errors() {
        let input = "---\ntitle: oops\nno closing dashes\n";
        assert!(parse(input).is_err());
    }

    #[test]
    fn yaml_inline_list_round_trips() {
        let original = vec!["a".to_string(), "b c".to_string(), "d, e".to_string()];
        let s = yaml_inline_list(&original);
        let parsed = parse_inline_list(&s);
        assert_eq!(parsed, original);
    }

    #[test]
    fn empty_list_renders_as_brackets() {
        assert_eq!(yaml_inline_list(&[]), "[]");
        assert!(parse_inline_list("[]").is_empty());
    }
}
