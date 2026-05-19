//! Shared helpers for the real-corpus benches (`corpus-code-bench`,
//! `corpus-memory-bench`): IR metrics, the PASS/FAIL/SKIP gate printer,
//! project/path normalization, and the `/api/v1/export` loader.
//!
//! Included via `#[path = "corpus_common.rs"] mod corpus_common;` from each
//! single-file `[[bin]]` (mirrors the existing one-file bench convention; no
//! lib target needed). Some items are only used by one bench.
#![allow(dead_code)]

use anyhow::{Context, Result};
use serde_json::Value;

// ---------------------------------------------------------------------------
// IR metrics (same definitions as benchmark/memory_bench.rs)
// ---------------------------------------------------------------------------

/// Fraction of probes whose oracle appears strictly within the top-`k`.
pub fn recall_at_k(ranks: &[Option<usize>], k: usize) -> f64 {
    if ranks.is_empty() {
        return 0.0;
    }
    let hits = ranks
        .iter()
        .filter(|r| matches!(r, Some(p) if *p < k))
        .count();
    hits as f64 / ranks.len() as f64
}

/// Mean reciprocal rank (0 when the oracle is absent).
pub fn mrr(ranks: &[Option<usize>]) -> f64 {
    if ranks.is_empty() {
        return 0.0;
    }
    let s: f64 = ranks
        .iter()
        .map(|r| match r {
            Some(p) => 1.0 / (*p as f64 + 1.0),
            None => 0.0,
        })
        .sum();
    s / ranks.len() as f64
}

// ---------------------------------------------------------------------------
// Gate printing — mirrors the `ground` gate style in memory_bench.rs:957.
// ---------------------------------------------------------------------------

pub enum Verdict {
    Pass,
    Fail(Vec<String>),
    Skip(String),
}

impl Verdict {
    /// Print `GATE: PASS|FAIL|SKIP` plus one reason line per failure, and
    /// return the process exit code (0 pass, 1 fail, 2 skip — distinct so CI
    /// can tell "broken" from "not enough data").
    pub fn emit(&self) -> i32 {
        match self {
            Verdict::Pass => {
                println!("GATE: PASS");
                0
            }
            Verdict::Fail(reasons) => {
                println!("GATE: FAIL");
                for r in reasons {
                    println!("  (reason: {r})");
                }
                1
            }
            Verdict::Skip(why) => {
                println!("GATE: SKIP  ({why})");
                2
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Project / path normalization.
//
// The real corpus stores `project` three different ways (verified via export):
// "hifz", "/Users/.../projects/hifz", and ".../projects/hifz/adapters/...".
// `run`/`session.project` are absolute paths; `memory.files` are repo-relative
// while observation `files` are absolute. These normalize both sides so the
// commit-grounded join actually intersects.
// ---------------------------------------------------------------------------

/// Canonical workspace project name: the path component after `projects/`,
/// else the last component, else the raw string — all lowercased.
pub fn norm_project(p: &str) -> String {
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(i) = parts.iter().position(|s| *s == "projects")
        && let Some(name) = parts.get(i + 1)
    {
        return name.to_lowercase();
    }
    parts
        .last()
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| p.to_lowercase())
}

/// Repo-relative, lowercased path. Strips any leading absolute prefix up to
/// and including `projects/<name>/`, then a leading `./`.
pub fn norm_path(p: &str) -> String {
    let lower = p.to_lowercase();
    let rel = if let Some(idx) = lower.find("/projects/") {
        // skip "/projects/<name>/"
        let after = &lower[idx + "/projects/".len()..];
        match after.find('/') {
            Some(s) => after[s + 1..].to_string(),
            None => after.to_string(),
        }
    } else {
        lower
    };
    rel.trim_start_matches("./").to_string()
}

pub fn basename(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_lowercase()
}

/// Do two file sets overlap? `loose` compares basenames only (fallback for
/// path-form mismatches the strict normalizer can't bridge).
pub fn files_overlap(a: &[String], b: &[String], loose: bool) -> bool {
    let key = |s: &String| if loose { basename(s) } else { norm_path(s) };
    let set: std::collections::HashSet<String> = a.iter().map(&key).collect();
    b.iter().any(|x| set.contains(&key(x)))
}

// ---------------------------------------------------------------------------
// /api/v1/export loader
// ---------------------------------------------------------------------------

pub struct Export {
    pub memories: Vec<Value>,
    pub runs: Vec<Value>,
    pub sessions: Vec<Value>,
    pub observations: Vec<Value>,
}

/// Load the corpus from a file (`--export-json`) or the live daemon
/// (`GET {base}/api/v1/export`). File mode keeps the bench reproducible and
/// daemon-free.
pub async fn load_export(export_json: Option<&str>, base_url: &str) -> Result<Export> {
    let raw: Value = match export_json {
        Some(path) => {
            let s = std::fs::read_to_string(path)
                .with_context(|| format!("reading --export-json {path}"))?;
            serde_json::from_str(&s).context("parsing export json file")?
        }
        None => {
            let url = format!("{base_url}/api/v1/export");
            let body = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?
                .get(&url)
                .send()
                .await
                .with_context(|| format!("GET {url} (is the daemon up? else pass --export-json)"))?
                .error_for_status()?
                .text()
                .await?;
            serde_json::from_str(&body).context("parsing export response")?
        }
    };
    let arr = |k: &str| -> Vec<Value> {
        raw.get(k)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    Ok(Export {
        memories: arr("memories"),
        runs: arr("runs"),
        sessions: arr("sessions"),
        observations: arr("observations"),
    })
}

/// Export serializes SurrealDB record ids as `"table:key"` strings; be
/// defensive about the occasional `{tb,id}`/`{"String":..}` shape too.
pub fn rec_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            if let Some(s) = o.get("String").and_then(|x| x.as_str()) {
                return Some(s.to_string());
            }
            let tb = o.get("tb").or_else(|| o.get("table"))?.as_str()?;
            let id = o.get("id")?;
            let id_s = id
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| id.get("String").and_then(|x| x.as_str()).map(String::from))
                .unwrap_or_else(|| id.to_string());
            Some(format!("{tb}:{id_s}"))
        }
        _ => None,
    }
}

pub fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(String::from)
}

pub fn strs_field(v: &Value, k: &str) -> Vec<String> {
    v.get(k)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// RFC3339 → epoch seconds (lenient; 0 on parse failure, matching how
/// `rank.rs` degrades unparseable dates to age 0).
pub fn epoch_secs(ts: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|d| d.timestamp())
        .unwrap_or(0)
}
