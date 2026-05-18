//! Source-language detection and tree-sitter grammar resolution.
//!
//! Extension-based mapping. Languages with a registered grammar return
//! `Some(tree_sitter::Language)`; everything else falls through to `Plain`
//! and uses the text-only fallback splitter.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Java,
    C,
    Cpp,
    /// Catch-all: no tree-sitter grammar; use text-only splitter.
    Plain,
}

impl Language {
    pub fn from_path(p: &Path) -> Option<Self> {
        let ext = p.extension()?.to_str()?;
        Self::from_ext(ext)
    }

    pub fn from_ext(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "py" | "pyi" => Self::Python,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "go" => Self::Go,
            "java" => Self::Java,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Self::Cpp,
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Go => "go",
            Self::Java => "java",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Plain => "text",
        }
    }

    /// Tree-sitter grammar handle. `None` for `Plain` — caller should fall back
    /// to the text-only splitter at `crate::chunk::split`.
    pub fn ts_language(&self) -> Option<tree_sitter::Language> {
        Some(match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Plain => return None,
        })
    }
}

/// Used by the walker to skip files we can't (or won't) chunk.
pub fn is_supported_extension(ext: &str) -> bool {
    Language::from_ext(ext).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ext_mapping_round_trip() {
        assert_eq!(Language::from_ext("rs"), Some(Language::Rust));
        assert_eq!(Language::from_ext("PY"), Some(Language::Python));
        assert_eq!(Language::from_ext("tsx"), Some(Language::Tsx));
        assert_eq!(Language::from_ext("toml"), None);
    }

    #[test]
    fn from_path_handles_no_extension() {
        assert_eq!(Language::from_path(&PathBuf::from("Makefile")), None);
        assert_eq!(
            Language::from_path(&PathBuf::from("src/main.rs")),
            Some(Language::Rust)
        );
    }

    #[test]
    fn ts_language_is_some_for_known() {
        assert!(Language::Rust.ts_language().is_some());
        assert!(Language::Plain.ts_language().is_none());
    }
}
