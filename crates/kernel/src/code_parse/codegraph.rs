//! The single code-intelligence walk: one imperative tree-sitter traversal
//! (no `.scm`) producing **semantically scope-qualified** definitions plus
//! the raw reference/import sites the resolver (`coderesolve`) later binds.
//!
//! Identity is the language-semantic fully-qualified path
//! (`crate::module::Type::method`), derived from the project module path
//! (`langmod`) + the in-file AST scope chain. Two same-named symbols in
//! different scopes are therefore distinct *by construction*.

use super::lang::Language;
use super::langcfg::LanguageConfig;

/// A defined symbol with its semantic qualified path and spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDef {
    /// Language-semantic fully-qualified path, e.g. `crate::a::Foo::run`.
    pub qualified: String,
    /// Bare identifier (`run`).
    pub name: String,
    /// `function|struct|enum|trait|method|const|module|class|interface|type|macro`.
    pub kind: String,
    /// Qualified path of the lexically enclosing symbol, if any
    /// (method → its impl/class; nested fn → outer fn). Enables the
    /// explicit `contains` edge + `parent_symbol` without string parsing.
    pub parent: Option<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1-indexed inclusive.
    pub start_line: usize,
    pub end_line: usize,
    /// First line of the definition (signature line) — best-effort.
    pub signature: Option<String>,
    /// Doc comment immediately preceding the definition (Rust `///`/`//!`,
    /// JS/TS/Java `/** */`, Go `//`, C/C++ `/** */`/`//`) or the Python
    /// docstring (first body string). `None` when undocumented.
    pub doc: Option<String>,
    /// Stable hash of the definition body — drives structural rename
    /// reconciliation in E4 (qualified changed but body identical → rename).
    pub body_hash: String,
}

/// A call/reference/import site to be bound by the resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefSite {
    /// Qualified path of the enclosing definition the reference occurs in
    /// (the edge source). `None` = file/module scope.
    pub from_qualified: Option<String>,
    /// The raw callee/imported name as written (`foo`, `obj.bar`, `a::b::c`).
    pub raw: String,
    pub kind: RefKind,
    pub start_line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Call,
    Import,
}

/// A parsed import/`use` binding: the name introduced into this file's
/// scope (`local`) and the full path it resolves to (`path`). `glob`
/// marks `use a::b::*` (no single local name — resolver falls back to the
/// name table for those, never silently dropping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBinding {
    pub local: String,
    pub path: String,
    pub glob: bool,
}

/// Full result of walking one file.
#[derive(Debug, Clone, Default)]
pub struct FileGraph {
    /// Semantic module prefix for this file (`crate::a::b`).
    pub module_path: String,
    pub defs: Vec<SymbolDef>,
    /// Call sites only (imports are structured into `imports`).
    pub refs: Vec<RefSite>,
    pub imports: Vec<ImportBinding>,
}

fn body_hash(src: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(src.as_bytes());
    hex::encode(&h.finalize()[..16])
}

/// Walk one parsed file. `module_path` is the semantic module prefix for
/// this file (from `langmod`), e.g. `crate::code::link`.
pub fn walk_file(lang: Language, source: &str, module_path: &str) -> anyhow::Result<FileGraph> {
    let cfg = LanguageConfig::for_lang(lang);
    let Some(cfg) = cfg else {
        return Ok(FileGraph::default());
    };
    let Some(ts_lang) = lang.ts_language() else {
        return Ok(FileGraph::default());
    };
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&ts_lang)?;
    let Some(tree) = parser.parse(source, None) else {
        return Ok(FileGraph::default());
    };

    let mut g = FileGraph {
        module_path: module_path.to_string(),
        ..FileGraph::default()
    };
    let mut scope: Vec<String> = vec![module_path.to_string()];
    walk_node(tree.root_node(), source, &cfg, &mut scope, None, &mut g);
    Ok(g)
}

#[allow(clippy::too_many_arguments)]
fn walk_node(
    node: tree_sitter::Node,
    src: &str,
    cfg: &LanguageConfig,
    scope: &mut Vec<String>,
    parent_qual: Option<String>,
    out: &mut FileGraph,
) {
    let kind = node.kind();

    // A scope-introducing node: either a named definition (fn/struct/trait/
    // mod/…) OR an `impl` block (no name of its own — its scope segment is
    // the Self type so methods qualify `Type::method`).
    let is_impl = cfg.impl_kinds.contains(&kind);
    let symkind = cfg.def_kind(kind);
    if symkind.is_some() || is_impl {
        let seg = if is_impl {
            cfg.impl_self_type(node, src)
        } else {
            cfg.def_name(node, src)
        };

        if let Some(seg) = seg {
            // Self-qualifying defs (Go method receiver) carry an extra
            // segment so same-named methods on different receivers are
            // distinct ids — the collision fix, language-aware.
            let qualified = match cfg.qualifier_prefix(node, src) {
                Some(p) => format!("{}::{}::{}", scope.join("::"), p, seg),
                None => format!("{}::{}", scope.join("::"), seg),
            };
            if let Some(sk) = symkind {
                // `impl` contributes a scope segment but emits no symbol.
                if !is_impl {
                    let sl = node.start_position().row + 1;
                    let el = node.end_position().row + 1;
                    let text = node.utf8_text(src.as_bytes()).unwrap_or("");
                    out.defs.push(SymbolDef {
                        qualified: qualified.clone(),
                        name: seg.clone(),
                        kind: sk.to_string(),
                        parent: parent_qual.clone(),
                        start_byte: node.start_byte(),
                        end_byte: node.end_byte(),
                        start_line: sl,
                        end_line: el,
                        signature: text.lines().next().map(|l| l.trim().to_string()),
                        doc: cfg.extract_doc(node, src),
                        // Hash the *body* subtree only (not the name/
                        // signature) so a pure rename keeps the same hash →
                        // structural rename reconciliation. Falls back to
                        // whole-node text for defs without a `body` field.
                        body_hash: body_hash(
                            node.child_by_field_name("body")
                                .and_then(|b| b.utf8_text(src.as_bytes()).ok())
                                .unwrap_or(text),
                        ),
                    });
                }
            }
            // Descend with this segment pushed onto the scope.
            scope.push(seg);
            let new_parent = Some(qualified);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_node(child, src, cfg, scope, new_parent.clone(), out);
            }
            scope.pop();
            return;
        }
    }

    // Imports → structured bindings (parsed from the AST, never by
    // string-splitting the `use` text).
    if cfg.is_import_kind(kind) {
        out.imports.extend(cfg.parse_imports(node, src));
        return;
    }

    // Call site (bound by the resolver).
    if let Some(RefKind::Call) = cfg.ref_kind(kind)
        && let Some(raw) = cfg.ref_name(node, src)
    {
        out.refs.push(RefSite {
            from_qualified: parent_qual.clone(),
            raw,
            kind: RefKind::Call,
            start_line: node.start_position().row + 1,
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(child, src, cfg, scope, parent_qual.clone(), out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_scope_qualified_distinct_for_same_name() {
        // Two `run` methods in different impl blocks + a free `run`.
        let src = r#"
struct A;
struct B;
impl A { fn run(&self) {} }
impl B { fn run(&self) {} }
fn run() {}
mod inner { fn run() {} }
"#;
        let g = walk_file(Language::Rust, src, "crate::demo").unwrap();
        let quals: Vec<&str> = g.defs.iter().map(|d| d.qualified.as_str()).collect();
        assert!(quals.contains(&"crate::demo::A::run"), "{quals:?}");
        assert!(quals.contains(&"crate::demo::B::run"), "{quals:?}");
        assert!(quals.contains(&"crate::demo::run"), "{quals:?}");
        assert!(quals.contains(&"crate::demo::inner::run"), "{quals:?}");
        // All four `run`s are distinct identities.
        let runs: Vec<_> = g.defs.iter().filter(|d| d.name == "run").collect();
        assert_eq!(runs.len(), 4);
        let uniq: std::collections::HashSet<_> = runs.iter().map(|d| d.qualified.clone()).collect();
        assert_eq!(uniq.len(), 4, "qualified paths must be unique");
    }

    #[test]
    fn rust_extracts_struct_enum_trait_fn() {
        let src = r#"
pub fn parse_chunk(s: &str) -> Vec<&str> { vec![s] }
pub struct Splitter { n: usize }
pub enum Mode { A, B }
pub trait Walk { fn go(&self); }
"#;
        let g = walk_file(Language::Rust, src, "crate").unwrap();
        let by = |n: &str| g.defs.iter().find(|d| d.name == n).map(|d| d.kind.as_str());
        assert_eq!(by("parse_chunk"), Some("function"));
        assert_eq!(by("Splitter"), Some("struct"));
        assert_eq!(by("Mode"), Some("enum"));
        assert_eq!(by("Walk"), Some("trait"));
        // Trait method qualifies under the trait.
        assert!(
            g.defs.iter().any(|d| d.qualified == "crate::Walk::go"),
            "{:?}",
            g.defs.iter().map(|d| &d.qualified).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rust_body_hash_stable_and_parent_set() {
        let src = "impl Foo { fn a(&self) { let x = 1; } }";
        let g1 = walk_file(Language::Rust, src, "crate").unwrap();
        let g2 = walk_file(Language::Rust, src, "crate").unwrap();
        let a1 = g1.defs.iter().find(|d| d.name == "a").unwrap();
        let a2 = g2.defs.iter().find(|d| d.name == "a").unwrap();
        assert_eq!(a1.body_hash, a2.body_hash, "deterministic");
        assert_eq!(a1.qualified, "crate::Foo::a");
        assert_eq!(a1.parent.as_deref(), Some("crate::Foo"));
    }
}
