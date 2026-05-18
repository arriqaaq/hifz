//! Per-language configuration driving the single generic walk in
//! `codegraph`. Data, not code, per language — new languages are added by
//! filling one of these in (E3c), not by forking the traversal.

use super::codegraph::RefKind;
use super::lang::Language;

pub struct LanguageConfig {
    /// tree-sitter field name carrying a definition's identifier.
    pub name_field: &'static str,
    /// Node kinds that introduce a *named* scope + emit a symbol, mapped to
    /// the canonical `kind` string (parity vocabulary with the old `.scm`).
    def_kinds: &'static [(&'static str, &'static str)],
    /// Node kinds that introduce a scope segment but emit no symbol of their
    /// own (Rust `impl` — its segment is the Self type).
    pub impl_kinds: &'static [&'static str],
    /// Call-site node kinds.
    call_kinds: &'static [&'static str],
    /// Import/use node kinds.
    import_kinds: &'static [&'static str],
    /// tree-sitter field on a call node carrying the callee.
    call_fn_field: &'static str,
    lang: Language,
}

impl LanguageConfig {
    pub fn for_lang(lang: Language) -> Option<LanguageConfig> {
        match lang {
            Language::Rust => Some(RUST),
            Language::Python => Some(PYTHON),
            Language::JavaScript => Some(JAVASCRIPT),
            Language::TypeScript | Language::Tsx => Some(TYPESCRIPT),
            Language::Go => Some(GO),
            Language::Java => Some(JAVA),
            Language::C => Some(C_LANG),
            Language::Cpp => Some(CPP),
            Language::Plain => None,
        }
    }

    pub fn def_kind(&self, node_kind: &str) -> Option<&'static str> {
        self.def_kinds
            .iter()
            .find(|(k, _)| *k == node_kind)
            .map(|(_, v)| *v)
    }

    pub fn ref_kind(&self, node_kind: &str) -> Option<RefKind> {
        if self.call_kinds.contains(&node_kind) {
            Some(RefKind::Call)
        } else if self.import_kinds.contains(&node_kind) {
            Some(RefKind::Import)
        } else {
            None
        }
    }

    pub fn is_import_kind(&self, node_kind: &str) -> bool {
        self.import_kinds.contains(&node_kind)
    }

    /// Parse an import/`use` node into structured bindings by walking the
    /// AST (Rust today; E3c per language). Never string-splits the whole
    /// statement.
    pub fn parse_imports(
        &self,
        node: tree_sitter::Node,
        src: &str,
    ) -> Vec<super::codegraph::ImportBinding> {
        match self.lang {
            Language::Rust => {
                let mut out = Vec::new();
                // `use_declaration` → its single `argument` clause.
                let arg = node.child_by_field_name("argument").or_else(|| {
                    // fall back: first named child that isn't the `use` kw
                    let mut c = node.walk();
                    node.named_children(&mut c).next()
                });
                if let Some(arg) = arg {
                    rust_use_clause(arg, "", src, &mut out);
                }
                out
            }
            Language::Python => {
                let mut out = Vec::new();
                python_import(node, src, &mut out);
                out
            }
            Language::JavaScript | Language::TypeScript | Language::Tsx => {
                let mut out = Vec::new();
                js_import(node, src, &mut out);
                out
            }
            Language::Java => {
                let mut out = Vec::new();
                if node.kind() == "import_declaration" {
                    let mut c = node.walk();
                    for ch in node.named_children(&mut c) {
                        match ch.kind() {
                            "scoped_identifier" | "identifier" => {
                                let p = ts_txt(ch, src).replace('.', "::");
                                out.push(super::codegraph::ImportBinding {
                                    local: last_seg(&p).to_string(),
                                    path: p,
                                    glob: false,
                                });
                            }
                            "asterisk" => out.push(super::codegraph::ImportBinding {
                                local: "*".into(),
                                path: "*".into(),
                                glob: true,
                            }),
                            _ => {}
                        }
                    }
                }
                out
            }
            Language::C | Language::Cpp => {
                let mut out = Vec::new();
                if node.kind() == "preproc_include" {
                    let mut c = node.walk();
                    if let Some(p) = node
                        .named_children(&mut c)
                        .find(|n| matches!(n.kind(), "string_literal" | "system_lib_string"))
                    {
                        let raw = ts_txt(p, src).trim_matches(['"', '<', '>']).to_string();
                        let local = std::path::Path::new(&raw)
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| raw.clone());
                        out.push(super::codegraph::ImportBinding {
                            local,
                            path: raw,
                            glob: false,
                        });
                    }
                }
                out
            }
            // Go cross-package import→file resolution is the same bounded
            // follow-up as JS specifier resolution; calls still resolve via
            // the name table (nothing dropped). Defs/identity unaffected.
            Language::Go => Vec::new(),
            Language::Plain => Vec::new(),
        }
    }

    /// Self type of an `impl` block (the scope segment its methods qualify
    /// under). Language-specific; only Rust uses `impl_kinds` today.
    pub fn impl_self_type(&self, node: tree_sitter::Node, src: &str) -> Option<String> {
        match self.lang {
            Language::Rust => node
                .child_by_field_name("type")
                .and_then(|n| n.utf8_text(src.as_bytes()).ok())
                .map(|s| s.trim().to_string()),
            _ => None,
        }
    }

    /// Definition identifier. Default = the `name` field; C/C++ drill the
    /// declarator chain (`int *foo()` / `void Ns::bar()`), matching the old
    /// `.scm`'s nested-declarator patterns.
    pub fn def_name(&self, node: tree_sitter::Node, src: &str) -> Option<String> {
        let txt = |n: tree_sitter::Node| {
            n.utf8_text(src.as_bytes())
                .ok()
                .map(|s| s.trim().to_string())
        };
        match self.lang {
            Language::C | Language::Cpp => match node.kind() {
                "function_definition" => {
                    let mut d = node.child_by_field_name("declarator")?;
                    loop {
                        match d.kind() {
                            "pointer_declarator" | "reference_declarator" => {
                                d = d.child_by_field_name("declarator")?;
                            }
                            "function_declarator" => {
                                d = d.child_by_field_name("declarator")?;
                                break;
                            }
                            _ => break,
                        }
                    }
                    txt(d)
                }
                "type_definition" => node.child_by_field_name("declarator").and_then(txt),
                _ => node.child_by_field_name(self.name_field).and_then(txt),
            },
            _ => node.child_by_field_name(self.name_field).and_then(txt),
        }
    }

    /// An extra qualifier segment a *self-qualifying* definition carries
    /// (Go method receiver type → `module::Recv::method`, so two `Close`
    /// methods on different receivers are distinct ids — the collision fix
    /// for Go). `None` for normal defs.
    pub fn qualifier_prefix(&self, node: tree_sitter::Node, src: &str) -> Option<String> {
        match self.lang {
            Language::Go if node.kind() == "method_declaration" => {
                // receiver: (parameter_list (parameter_declaration type: …))
                let recv = node.child_by_field_name("receiver")?;
                let mut c = recv.walk();
                let pd = recv
                    .named_children(&mut c)
                    .find(|n| n.kind() == "parameter_declaration")?;
                let ty = pd.child_by_field_name("type")?;
                // strip leading `*` on pointer receivers
                let t = ty.utf8_text(src.as_bytes()).ok()?.trim();
                Some(t.trim_start_matches('*').trim().to_string())
            }
            _ => None,
        }
    }

    /// The raw callee / imported path text for a reference node.
    pub fn ref_name(&self, node: tree_sitter::Node, src: &str) -> Option<String> {
        if self.call_kinds.contains(&node.kind()) {
            let callee = node.child_by_field_name(self.call_fn_field)?;
            // `a.b()` → `b`; `a::b()` / `b()` → full text.
            let txt = match callee.kind() {
                // Rust `a.b()` → `b`; Python `a.b()` → `b`.
                "field_expression" => callee
                    .child_by_field_name("field")
                    .and_then(|n| n.utf8_text(src.as_bytes()).ok())?,
                "attribute" => callee
                    .child_by_field_name("attribute")
                    .and_then(|n| n.utf8_text(src.as_bytes()).ok())?,
                // JS/TS `a.b()` → `b`.
                "member_expression" => callee
                    .child_by_field_name("property")
                    .and_then(|n| n.utf8_text(src.as_bytes()).ok())?,
                _ => callee.utf8_text(src.as_bytes()).ok()?,
            };
            return Some(txt.trim().to_string());
        }
        if self.import_kinds.contains(&node.kind()) {
            // The whole `use a::b::c;` argument text — resolver parses it.
            return node
                .utf8_text(src.as_bytes())
                .ok()
                .map(|s| s.trim().trim_end_matches(';').to_string());
        }
        None
    }
}

fn ts_txt<'a>(n: tree_sitter::Node, src: &'a str) -> &'a str {
    n.utf8_text(src.as_bytes()).unwrap_or("").trim()
}
fn join_path(prefix: &str, seg: &str) -> String {
    if prefix.is_empty() {
        seg.to_string()
    } else if seg.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}::{seg}")
    }
}
fn last_seg(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// Recursively expand a Rust `use` clause into flat bindings. Reads the
/// tree-sitter AST (fields/kinds), not the raw statement text.
fn rust_use_clause(
    node: tree_sitter::Node,
    prefix: &str,
    src: &str,
    out: &mut Vec<super::codegraph::ImportBinding>,
) {
    use super::codegraph::ImportBinding;
    match node.kind() {
        "scoped_identifier"
        | "identifier"
        | "crate"
        | "self"
        | "super"
        | "scoped_type_identifier"
        | "type_identifier" => {
            let full = join_path(prefix, ts_txt(node, src));
            if !full.is_empty() {
                out.push(ImportBinding {
                    local: last_seg(&full).to_string(),
                    path: full,
                    glob: false,
                });
            }
        }
        "use_as_clause" => {
            let p = node
                .child_by_field_name("path")
                .map(|n| join_path(prefix, ts_txt(n, src)))
                .unwrap_or_default();
            let local = node
                .child_by_field_name("alias")
                .map(|a| ts_txt(a, src).to_string())
                .unwrap_or_else(|| last_seg(&p).to_string());
            out.push(ImportBinding {
                local,
                path: p,
                glob: false,
            });
        }
        "scoped_use_list" => {
            let p = node
                .child_by_field_name("path")
                .map(|n| join_path(prefix, ts_txt(n, src)))
                .unwrap_or_else(|| prefix.to_string());
            if let Some(list) = node.child_by_field_name("list") {
                rust_use_clause(list, &p, src, out);
            }
        }
        "use_list" => {
            let mut c = node.walk();
            for child in node.named_children(&mut c) {
                rust_use_clause(child, prefix, src, out);
            }
        }
        "use_wildcard" => {
            let mut c = node.walk();
            let p = node
                .named_children(&mut c)
                .next()
                .map(|n| join_path(prefix, ts_txt(n, src)))
                .unwrap_or_else(|| prefix.to_string());
            out.push(ImportBinding {
                local: "*".into(),
                path: p,
                glob: true,
            });
        }
        _ => {
            let t = ts_txt(node, src);
            if !t.is_empty() {
                let full = join_path(prefix, t);
                out.push(ImportBinding {
                    local: last_seg(&full).to_string(),
                    path: full,
                    glob: false,
                });
            }
        }
    }
}

/// Python `import` / `from … import …` → structured bindings (AST-driven).
/// Internal paths use the canonical `::` separator (identity is internal;
/// Python's surface `.` is normalized).
fn python_import(
    node: tree_sitter::Node,
    src: &str,
    out: &mut Vec<super::codegraph::ImportBinding>,
) {
    use super::codegraph::ImportBinding;
    let dot_to_sep = |s: &str| s.replace('.', "::");
    match node.kind() {
        "import_statement" => {
            let mut c = node.walk();
            for child in node.named_children(&mut c) {
                match child.kind() {
                    "dotted_name" => {
                        let full = dot_to_sep(ts_txt(child, src));
                        out.push(ImportBinding {
                            local: last_seg(&full).to_string(),
                            path: full,
                            glob: false,
                        });
                    }
                    "aliased_import" => {
                        let name = child
                            .child_by_field_name("name")
                            .map(|n| dot_to_sep(ts_txt(n, src)))
                            .unwrap_or_default();
                        let alias = child
                            .child_by_field_name("alias")
                            .map(|a| ts_txt(a, src).to_string())
                            .unwrap_or_else(|| last_seg(&name).to_string());
                        out.push(ImportBinding {
                            local: alias,
                            path: name,
                            glob: false,
                        });
                    }
                    _ => {}
                }
            }
        }
        "import_from_statement" => {
            let base = node
                .child_by_field_name("module_name")
                .map(|m| {
                    if m.kind() == "relative_import" {
                        // keep relative marker; resolver falls back to the
                        // name table (still resolves unique internal names).
                        ts_txt(m, src).replace('.', "::")
                    } else {
                        dot_to_sep(ts_txt(m, src))
                    }
                })
                .unwrap_or_default();
            let mut c = node.walk();
            for child in node.named_children(&mut c) {
                match child.kind() {
                    "wildcard_import" => out.push(ImportBinding {
                        local: "*".into(),
                        path: base.clone(),
                        glob: true,
                    }),
                    "dotted_name" | "identifier" => {
                        // skip the module_name node itself (already consumed)
                        if Some(child.id())
                            == node.child_by_field_name("module_name").map(|n| n.id())
                        {
                            continue;
                        }
                        let nm = ts_txt(child, src);
                        out.push(ImportBinding {
                            local: last_seg(nm).to_string(),
                            path: join_path(&base, &dot_to_sep(nm)),
                            glob: false,
                        });
                    }
                    "aliased_import" => {
                        let nm = child
                            .child_by_field_name("name")
                            .map(|n| ts_txt(n, src).to_string())
                            .unwrap_or_default();
                        let alias = child
                            .child_by_field_name("alias")
                            .map(|a| ts_txt(a, src).to_string())
                            .unwrap_or_else(|| last_seg(&nm).to_string());
                        out.push(ImportBinding {
                            local: alias,
                            path: join_path(&base, &dot_to_sep(&nm)),
                            glob: false,
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

const PYTHON: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("function_definition", "function"),
        ("class_definition", "class"),
    ],
    impl_kinds: &[],
    call_kinds: &["call"],
    import_kinds: &["import_statement", "import_from_statement"],
    call_fn_field: "function",
    lang: Language::Python,
};

/// ESM `import` → structured bindings (AST-driven). Module specifiers are
/// kept as written (`./db`, `react`); cross-module *specifier→file*
/// resolution is the bounded JS/TS deepening — until then the resolver's
/// name-table fallback handles internal names and bare specifiers resolve
/// to External (correct: npm deps). 3-state contract holds; nothing dropped.
fn js_import(node: tree_sitter::Node, src: &str, out: &mut Vec<super::codegraph::ImportBinding>) {
    use super::codegraph::ImportBinding;
    let spec = node
        .child_by_field_name("source")
        .map(|s| ts_txt(s, src).trim_matches(['"', '\'', '`']).to_string())
        .unwrap_or_default();
    if spec.is_empty() {
        return;
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        if child.kind() != "import_clause" {
            continue;
        }
        let mut cc = child.walk();
        for part in child.named_children(&mut cc) {
            match part.kind() {
                "identifier" => out.push(ImportBinding {
                    local: ts_txt(part, src).to_string(),
                    path: spec.clone(),
                    glob: false,
                }),
                "namespace_import" => {
                    let mut ic = part.walk();
                    if let Some(id) = part
                        .named_children(&mut ic)
                        .find(|n| n.kind() == "identifier")
                    {
                        out.push(ImportBinding {
                            local: ts_txt(id, src).to_string(),
                            path: spec.clone(),
                            glob: false,
                        });
                    }
                }
                "named_imports" => {
                    let mut nc = part.walk();
                    for isp in part.named_children(&mut nc) {
                        if isp.kind() != "import_specifier" {
                            continue;
                        }
                        let name = isp
                            .child_by_field_name("name")
                            .map(|n| ts_txt(n, src).to_string())
                            .unwrap_or_default();
                        let alias = isp
                            .child_by_field_name("alias")
                            .map(|a| ts_txt(a, src).to_string());
                        if name.is_empty() {
                            continue;
                        }
                        out.push(ImportBinding {
                            local: alias.unwrap_or_else(|| name.clone()),
                            path: spec.clone(),
                            glob: false,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

const JAVASCRIPT: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("function_declaration", "function"),
        ("generator_function_declaration", "function"),
        ("class_declaration", "class"),
        ("method_definition", "method"),
    ],
    impl_kinds: &[],
    call_kinds: &["call_expression"],
    import_kinds: &["import_statement"],
    call_fn_field: "function",
    lang: Language::JavaScript,
};

const GO: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("function_declaration", "function"),
        ("method_declaration", "method"),
        ("type_spec", "type"),
    ],
    impl_kinds: &[],
    call_kinds: &["call_expression"],
    import_kinds: &["import_declaration"],
    call_fn_field: "function",
    lang: Language::Go,
};

const JAVA: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("class_declaration", "class"),
        ("interface_declaration", "interface"),
        ("enum_declaration", "enum"),
        ("method_declaration", "method"),
        ("constructor_declaration", "method"),
    ],
    impl_kinds: &[],
    call_kinds: &["method_invocation"],
    import_kinds: &["import_declaration"],
    call_fn_field: "name",
    lang: Language::Java,
};

const C_LANG: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("function_definition", "function"),
        ("struct_specifier", "struct"),
        ("enum_specifier", "enum"),
        ("type_definition", "type"),
    ],
    impl_kinds: &[],
    call_kinds: &["call_expression"],
    import_kinds: &["preproc_include"],
    call_fn_field: "function",
    lang: Language::C,
};

const CPP: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("function_definition", "function"),
        ("class_specifier", "class"),
        ("struct_specifier", "struct"),
        ("enum_specifier", "enum"),
        ("namespace_definition", "namespace"),
    ],
    impl_kinds: &[],
    call_kinds: &["call_expression"],
    import_kinds: &["preproc_include"],
    call_fn_field: "function",
    lang: Language::Cpp,
};

const TYPESCRIPT: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("function_declaration", "function"),
        ("generator_function_declaration", "function"),
        ("class_declaration", "class"),
        ("abstract_class_declaration", "class"),
        ("method_definition", "method"),
        ("method_signature", "method"),
        ("interface_declaration", "interface"),
        ("type_alias_declaration", "type"),
        ("enum_declaration", "enum"),
    ],
    impl_kinds: &[],
    call_kinds: &["call_expression"],
    import_kinds: &["import_statement"],
    call_fn_field: "function",
    lang: Language::TypeScript,
};

const RUST: LanguageConfig = LanguageConfig {
    name_field: "name",
    def_kinds: &[
        ("function_item", "function"),
        ("function_signature_item", "function"), // trait method sigs (superset vs old .scm)
        ("struct_item", "struct"),
        ("union_item", "struct"),
        ("enum_item", "enum"),
        ("trait_item", "trait"),
        ("type_item", "type"),
        ("mod_item", "module"),
        ("const_item", "const"),
        ("static_item", "const"),
        ("macro_definition", "macro"),
    ],
    impl_kinds: &["impl_item"],
    call_kinds: &["call_expression", "macro_invocation"],
    import_kinds: &["use_declaration"],
    call_fn_field: "function",
    lang: Language::Rust,
};
