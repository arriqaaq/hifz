//! Reference binding. Every call/import is bound to exactly one outcome —
//! `Resolved` (one indexed symbol), `External` (outside the indexed set), or
//! `Ambiguous` (undecidable, candidates recorded). **Nothing is dropped.**
//!
//! E3 baseline: scope-preferring unique-match resolution + explicit
//! External/Ambiguous (already strictly better than the reference tool,
//! which silently drops ambiguous cross-file calls). E3b deepens Rust
//! precision with the real `use`/import table + lexical shadowing; E3c
//! extends per language. The *contract* (three honest outcomes, no loss)
//! is final now; only intra-`Resolved`/`Ambiguous` precision deepens.

use std::collections::HashMap;

use super::codegraph::{FileGraph, SymbolDef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Resolved,
    External,
    Ambiguous,
}

impl Resolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Resolution::Resolved => "resolved",
            Resolution::External => "external",
            Resolution::Ambiguous => "ambiguous",
        }
    }
}

/// One bound graph edge. `to` is a symbol qualified path for `Resolved`,
/// a canonical external key for `External`, or the chosen-none marker for
/// `Ambiguous` (with `candidates` populated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundEdge {
    pub from: String,
    pub to: String,
    /// `calls` | `imports` | `contains`.
    pub relation: &'static str,
    pub resolution: Resolution,
    /// Populated only when `resolution == Ambiguous`.
    pub candidates: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectGraph {
    pub defs: Vec<SymbolDef>,
    pub edges: Vec<BoundEdge>,
}

/// Bind all references across the project. `files` = the per-file walk
/// output (already scope-qualified by `codegraph`).
pub fn resolve_project(files: Vec<FileGraph>) -> ProjectGraph {
    let mut out = ProjectGraph::default();
    let mut by_qualified: HashMap<String, ()> = HashMap::new();
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();

    for fg in &files {
        for d in &fg.defs {
            by_qualified.insert(d.qualified.clone(), ());
            by_name
                .entry(d.name.clone())
                .or_default()
                .push(d.qualified.clone());
        }
    }
    for v in by_name.values_mut() {
        v.sort();
        v.dedup();
    }

    for fg in files {
        // Per-file import/alias table (non-glob). `local → full path`.
        let aliases: HashMap<&str, &str> = fg
            .imports
            .iter()
            .filter(|b| !b.glob)
            .map(|b| (b.local.as_str(), b.path.as_str()))
            .collect();

        // Structural containment — always Resolved (parent path is
        // constructed by the walk, guaranteed to exist as a def).
        for d in &fg.defs {
            if let Some(parent) = &d.parent {
                out.edges.push(BoundEdge {
                    from: parent.clone(),
                    to: d.qualified.clone(),
                    relation: "contains",
                    resolution: Resolution::Resolved,
                    candidates: vec![],
                });
            }
        }

        // Import edges: file/module → target (Resolved if the path names an
        // indexed symbol/module, else External — most 3rd-party crates).
        for b in &fg.imports {
            let (to, res) = if by_qualified.contains_key(&b.path) {
                (b.path.clone(), Resolution::Resolved)
            } else {
                match unique_suffix_match(&b.path, &by_name) {
                    Some(q) => (q, Resolution::Resolved),
                    None => (b.path.clone(), Resolution::External),
                }
            };
            out.edges.push(BoundEdge {
                from: fg.module_path.clone(),
                to,
                relation: "imports",
                resolution: res,
                candidates: vec![],
            });
        }

        for r in fg.refs {
            let from = r
                .from_qualified
                .clone()
                .unwrap_or_else(|| fg.module_path.clone());
            let first = r.raw.split("::").next().unwrap_or(&r.raw).trim();
            let last = r.raw.rsplit("::").next().unwrap_or(&r.raw).trim();

            let (to, res, cands) = resolve_call(
                &r.raw,
                first,
                last,
                &fg.module_path,
                &aliases,
                &by_qualified,
                &by_name,
            );
            out.edges.push(BoundEdge {
                from,
                to,
                relation: "calls",
                resolution: res,
                candidates: cands,
            });
        }
        out.defs.extend(fg.defs);
    }
    out
}

/// Correct resolution order: lexical/same-module shadowing → exact
/// qualified → `use` alias substitution → scope-preferring name table.
/// Always returns one definite outcome — never drops.
#[allow(clippy::type_complexity)]
fn resolve_call(
    raw: &str,
    first: &str,
    last: &str,
    caller_module: &str,
    aliases: &HashMap<&str, &str>,
    by_qualified: &HashMap<String, ()>,
    by_name: &HashMap<String, Vec<String>>,
) -> (String, Resolution, Vec<String>) {
    // 1. Lexical/same-module: a def named `last` in the caller's own module
    //    shadows any import (Rust name resolution). Precise: the candidate's
    //    qualified path is exactly `<caller_module>::<last>`.
    let same_module = format!("{caller_module}::{last}");
    if by_qualified.contains_key(&same_module) {
        return (same_module, Resolution::Resolved, vec![]);
    }

    // 2. Exact qualified path as written.
    if by_qualified.contains_key(raw) {
        return (raw.to_string(), Resolution::Resolved, vec![]);
    }

    // 3. `use` alias substitution: rewrite leading segment via the import
    //    table, then require an exact hit (precise — no guessing).
    if let Some(base) = aliases.get(first) {
        let rest = raw
            .strip_prefix(first)
            .unwrap_or("")
            .trim_start_matches(':');
        let candidate = if rest.is_empty() {
            (*base).to_string()
        } else {
            format!("{base}::{rest}")
        };
        if by_qualified.contains_key(&candidate) {
            return (candidate, Resolution::Resolved, vec![]);
        }
        // Imported name itself is the target (e.g. `use a::foo; foo()`).
        if by_qualified.contains_key(*base) {
            return ((*base).to_string(), Resolution::Resolved, vec![]);
        }
    }

    // 4. Scope-preferring name table.
    let cands = by_name.get(last).cloned().unwrap_or_default();
    match cands.len() {
        0 => (raw.to_string(), Resolution::External, vec![]),
        1 => (cands[0].clone(), Resolution::Resolved, vec![]),
        _ => match pick_by_scope(caller_module, &cands) {
            Some(q) => (q, Resolution::Resolved, vec![]),
            None => (String::new(), Resolution::Ambiguous, cands),
        },
    }
}

/// A path whose tail uniquely matches one indexed symbol's qualified path.
fn unique_suffix_match(path: &str, by_name: &HashMap<String, Vec<String>>) -> Option<String> {
    let last = path.rsplit("::").next().unwrap_or(path);
    let cands = by_name.get(last)?;
    let hits: Vec<&String> = cands.iter().filter(|q| q.ends_with(path)).collect();
    if hits.len() == 1 {
        Some(hits[0].clone())
    } else {
        None
    }
}

/// Unique longest-shared-module-prefix winner, else `None` (truly
/// ambiguous). Conservative: only auto-picks when exactly one candidate has
/// the maximal shared prefix — never guesses between equals.
fn pick_by_scope(from: &str, cands: &[String]) -> Option<String> {
    fn shared(a: &str, b: &str) -> usize {
        a.split("::")
            .zip(b.split("::"))
            .take_while(|(x, y)| x == y)
            .count()
    }
    let mut scored: Vec<(usize, &String)> = cands.iter().map(|c| (shared(from, c), c)).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    if scored.len() >= 2 && scored[0].0 == scored[1].0 {
        return None; // tie → genuinely ambiguous
    }
    scored.first().map(|(_, c)| (*c).clone())
}

#[cfg(test)]
mod tests {
    use super::super::codegraph::walk_file;
    use super::super::lang::Language;
    use super::*;

    #[test]
    fn nothing_dropped_states_all_represented() {
        // `local()` resolves; `Missing::gone()` is external; two `dup`
        // in sibling modules with a caller equidistant → ambiguous.
        let src = r#"
mod a { fn dup() {} }
mod b { fn dup() {} }
fn local() {}
fn caller() {
    local();
    Missing::gone();
}
fn pick() { dup(); }
"#;
        let fg = walk_file(Language::Rust, src, "crate").unwrap();
        let g = resolve_project(vec![fg]);
        let calls: Vec<_> = g.edges.iter().filter(|e| e.relation == "calls").collect();
        // every call produced exactly one edge with a definite resolution.
        assert!(
            calls
                .iter()
                .any(|e| e.to == "crate::local" && e.resolution == Resolution::Resolved)
        );
        assert!(
            calls
                .iter()
                .any(|e| e.resolution == Resolution::External && e.to.contains("gone"))
        );
        assert!(
            calls
                .iter()
                .any(|e| e.resolution == Resolution::Ambiguous && e.candidates.len() == 2)
        );
        // contains edges for the two modules' fns.
        assert!(
            g.edges
                .iter()
                .any(|e| e.relation == "contains" && e.to == "crate::a::dup")
        );
    }

    #[test]
    fn use_alias_resolves_precisely() {
        // Caller crate imports `Db` from another module and calls `Db::open`.
        let dbfile = walk_file(
            Language::Rust,
            "pub struct Db; impl Db { pub fn open() {} }",
            "cratename::db",
        )
        .unwrap();
        let caller = walk_file(
            Language::Rust,
            "use cratename::db::Db;\nfn go() { Db::open(); }",
            "cratename::app",
        )
        .unwrap();
        let g = resolve_project(vec![dbfile, caller]);
        // `Db::open()` → cratename::db::Db::open via the use-alias table.
        assert!(
            g.edges.iter().any(|e| e.relation == "calls"
                && e.to == "cratename::db::Db::open"
                && e.resolution == Resolution::Resolved),
            "calls: {:?}",
            g.edges
                .iter()
                .filter(|e| e.relation == "calls")
                .collect::<Vec<_>>()
        );
        // The `use` itself is an imports edge, Resolved (Db is indexed).
        assert!(g.edges.iter().any(|e| e.relation == "imports"
            && e.to == "cratename::db::Db"
            && e.resolution == Resolution::Resolved));
    }

    #[test]
    fn python_class_method_scope_and_import() {
        let lib = walk_file(
            Language::Python,
            "class Db:\n    def open(self):\n        pass\n",
            "pkg::db",
        )
        .unwrap();
        let app = walk_file(
            Language::Python,
            "from pkg.db import Db\n\ndef go():\n    d = Db()\n    d.open()\n",
            "pkg::app",
        )
        .unwrap();
        let g = resolve_project(vec![lib, app]);
        assert!(
            g.defs
                .iter()
                .any(|d| d.qualified == "pkg::db::Db" && d.kind == "class"),
            "defs: {:?}",
            g.defs.iter().map(|d| &d.qualified).collect::<Vec<_>>()
        );
        assert!(
            g.defs
                .iter()
                .any(|d| d.qualified == "pkg::db::Db::open" && d.kind == "function")
        );
        assert!(g.edges.iter().any(|e| e.relation == "imports"
            && e.to == "pkg::db::Db"
            && e.resolution == Resolution::Resolved));
        assert!(
            g.edges.iter().any(|e| e.relation == "calls"
                && e.to == "pkg::db::Db::open"
                && e.resolution == Resolution::Resolved),
            "calls: {:?}",
            g.edges
                .iter()
                .filter(|e| e.relation == "calls")
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn js_class_method_scope_and_call() {
        let lib = walk_file(
            Language::JavaScript,
            "export class Db { open() { return 1; } }",
            "src::db",
        )
        .unwrap();
        let app = walk_file(
            Language::JavaScript,
            "import { Db } from './db';\nfunction go() { const d = new Db(); d.open(); }",
            "src::app",
        )
        .unwrap();
        let g = resolve_project(vec![lib, app]);
        assert!(
            g.defs
                .iter()
                .any(|d| d.qualified == "src::db::Db" && d.kind == "class"),
            "defs: {:?}",
            g.defs.iter().map(|d| &d.qualified).collect::<Vec<_>>()
        );
        assert!(
            g.defs
                .iter()
                .any(|d| d.qualified == "src::db::Db::open" && d.kind == "method")
        );
        // d.open() → unique name → Resolved (3-state contract, nothing dropped)
        assert!(
            g.edges.iter().any(|e| e.relation == "calls"
                && e.to == "src::db::Db::open"
                && e.resolution == Resolution::Resolved),
            "calls: {:?}",
            g.edges
                .iter()
                .filter(|e| e.relation == "calls")
                .collect::<Vec<_>>()
        );
        // the `import { Db } from './db'` is recorded (not dropped).
        assert!(g.edges.iter().any(|e| e.relation == "imports"));
    }

    #[test]
    fn external_import_is_external_not_dropped() {
        let caller = walk_file(
            Language::Rust,
            "use serde::Serialize;\nfn f() {}",
            "cratename::app",
        )
        .unwrap();
        let g = resolve_project(vec![caller]);
        assert!(g.edges.iter().any(|e| e.relation == "imports"
            && e.to == "serde::Serialize"
            && e.resolution == Resolution::External));
    }
}
