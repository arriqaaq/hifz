//! Symbol extraction via tree-sitter Query.
//!
//! For each language we ship a `.scm` query file at `ts_queries/<lang>.scm`.
//! Patterns capture `@name` (the identifier) and `@def.<kind>` (the whole
//! defining node). The kind label is the suffix after `def.` — e.g. a match
//! tagged `@def.function` produces a `RawSymbol` with `kind="function"`.
//!
//! ## Qualified names (M1)
//! `qualified == name` for now. M4 walks the parent chain to derive proper
//! `module::name` / `Class::method` paths. M2's indexer wipes-and-recreates
//! all symbols per file, so duplicate `qualified` values within a file are
//! tolerated by the schema (no UNIQUE constraint at v1).

use anyhow::{Context, Result};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

use crate::code::lang::Language;

const RUST_QUERY: &str = include_str!("ts_queries/rust.scm");
const PYTHON_QUERY: &str = include_str!("ts_queries/python.scm");
const JAVASCRIPT_QUERY: &str = include_str!("ts_queries/javascript.scm");
const TYPESCRIPT_QUERY: &str = include_str!("ts_queries/typescript.scm");
const GO_QUERY: &str = include_str!("ts_queries/go.scm");
const JAVA_QUERY: &str = include_str!("ts_queries/java.scm");
const C_QUERY: &str = include_str!("ts_queries/c.scm");
const CPP_QUERY: &str = include_str!("ts_queries/cpp.scm");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSymbol {
    pub name: String,
    /// M1: equals `name`. M4 will derive `module::name` from the AST chain.
    pub qualified: String,
    /// `function` | `struct` | `enum` | `trait` | `method` | `const` |
    /// `module` | `class` | `interface` | `type` | `namespace` | `macro`
    pub kind: String,
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1-indexed inclusive
    pub start_line: usize,
    /// 1-indexed inclusive
    pub end_line: usize,
}

fn query_for(lang: Language) -> Option<&'static str> {
    Some(match lang {
        Language::Rust => RUST_QUERY,
        Language::Python => PYTHON_QUERY,
        Language::JavaScript => JAVASCRIPT_QUERY,
        Language::TypeScript | Language::Tsx => TYPESCRIPT_QUERY,
        Language::Go => GO_QUERY,
        Language::Java => JAVA_QUERY,
        Language::C => C_QUERY,
        Language::Cpp => CPP_QUERY,
        Language::Plain => return None,
    })
}

pub fn extract_symbols(lang: Language, source: &str) -> Result<Vec<RawSymbol>> {
    let Some(ts_lang) = lang.ts_language() else {
        return Ok(Vec::new());
    };
    let Some(query_src) = query_for(lang) else {
        return Ok(Vec::new());
    };

    let mut parser = Parser::new();
    parser
        .set_language(&ts_lang)
        .context("set_language failed")?;
    let Some(tree) = parser.parse(source, None) else {
        return Ok(Vec::new());
    };

    let query = Query::new(&ts_lang, query_src).context("query parse failed")?;
    let capture_names: Vec<&str> = query.capture_names().iter().copied().collect();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    let mut out: Vec<RawSymbol> = Vec::new();

    while let Some(m) = matches.next() {
        // A pattern produces two captures: @name and @def.<kind>. We collect
        // both, then emit one RawSymbol per match.
        let mut name_text: Option<String> = None;
        let mut kind: Option<String> = None;
        let mut def_start_byte: usize = 0;
        let mut def_end_byte: usize = 0;
        let mut def_start_line: usize = 0;
        let mut def_end_line: usize = 0;
        let mut def_seen = false;

        for cap in m.captures {
            let cap_name = capture_names
                .get(cap.index as usize)
                .copied()
                .unwrap_or("");
            if cap_name == "name" {
                let txt = cap
                    .node
                    .utf8_text(source.as_bytes())
                    .unwrap_or("")
                    .to_string();
                if !txt.is_empty() {
                    name_text = Some(txt);
                }
            } else if let Some(suffix) = cap_name.strip_prefix("def.") {
                kind = Some(suffix.to_string());
                def_start_byte = cap.node.start_byte();
                def_end_byte = cap.node.end_byte();
                def_start_line = cap.node.start_position().row + 1;
                def_end_line = cap.node.end_position().row + 1;
                def_seen = true;
            }
        }

        let (Some(name), Some(kind)) = (name_text, kind) else {
            continue;
        };
        if !def_seen {
            continue;
        }

        out.push(RawSymbol {
            name: name.clone(),
            qualified: name,
            kind,
            start_byte: def_start_byte,
            end_byte: def_end_byte,
            start_line: def_start_line,
            end_line: def_end_line,
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_extracts_fn_struct_enum_trait() {
        let src = r#"
pub fn parse_chunk(s: &str) -> Vec<&str> { vec![s] }

pub struct Embedder { dim: usize }

pub enum Direction { In, Out }

pub trait Splitter { fn split(&self); }

const MAX: usize = 16;
"#;
        let syms = extract_symbols(Language::Rust, src).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"parse_chunk"));
        assert!(names.contains(&"Embedder"));
        assert!(names.contains(&"Direction"));
        assert!(names.contains(&"Splitter"));
        assert!(names.contains(&"MAX"));

        // Sanity: the function symbol has kind="function" and a sensible line range.
        let fn_sym = syms.iter().find(|s| s.name == "parse_chunk").unwrap();
        assert_eq!(fn_sym.kind, "function");
        assert!(fn_sym.start_line >= 2);
        assert!(fn_sym.end_line >= fn_sym.start_line);
    }

    #[test]
    fn python_extracts_function_and_class() {
        let src = "class Foo:\n    def bar(self):\n        return 1\n\ndef top():\n    return 2\n";
        let syms = extract_symbols(Language::Python, src).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"));
        assert!(names.contains(&"top"));
        // bar is a function_definition inside the class — also captured.
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn javascript_class_and_function() {
        let src = "function add(a, b) { return a + b; }\nclass Box { open() {} }\n";
        let syms = extract_symbols(Language::JavaScript, src).unwrap();
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"));
        assert!(names.contains(&"Box"));
    }

    #[test]
    fn empty_or_plain_yields_no_symbols() {
        assert!(extract_symbols(Language::Rust, "").unwrap().is_empty());
        assert!(extract_symbols(Language::Plain, "anything").unwrap().is_empty());
    }
}
