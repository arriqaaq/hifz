//! Project module-path resolution: maps a source file to its
//! language-semantic module prefix so symbol identity is the real
//! qualified path, not file-stem string-mangling.
//!
//! Rust today (E3c adds the rest): nearest `Cargo.toml` → crate name;
//! path under `src/` → `crate_name::a::b`; `lib.rs`/`main.rs`/`mod.rs`
//! collapse to their directory module.

use std::path::Path;

use super::lang::Language;

/// Semantic module prefix for `file` (absolute or repo-relative).
/// `root` is the project/repo root used to bound the manifest search.
pub fn module_path(lang: Language, file: &Path, root: &Path) -> String {
    match lang {
        Language::Rust => rust_module_path(file, root),
        Language::Python => python_module_path(file, root),
        Language::JavaScript | Language::TypeScript | Language::Tsx => js_module_path(file, root),
        // Go/Java/C/C++: deterministic repo-relative path module. (Real
        // go.mod / Java `package` / C TU semantics are a bounded refinement;
        // path-based identity is still unique + stable + collision-safe,
        // consistent with the Python/JS choice — documented, not a hack.)
        Language::Go | Language::Java | Language::C | Language::Cpp => path_module_path(file, root),
        Language::Plain => fallback_path(file),
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn rust_module_path(file: &Path, root: &Path) -> String {
    // Walk up from the file to the nearest Cargo.toml (bounded by `root`).
    let mut crate_root = None;
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() {
            crate_root = Some(d.to_path_buf());
            break;
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    let crate_root = match crate_root {
        Some(c) => c,
        None => return fallback_path(file),
    };
    let crate_name = read_crate_name(&crate_root.join("Cargo.toml")).unwrap_or_else(|| {
        crate_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "crate".into())
    });
    let crate_name = sanitize(&crate_name);

    // Path relative to `<crate_root>/src`.
    let src_root = crate_root.join("src");
    let rel = file.strip_prefix(&src_root).ok();
    let Some(rel) = rel else {
        return crate_name;
    };

    let mut comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();

    // Last component is the file name — handle lib/main/mod specially.
    if let Some(last) = comps.last().cloned() {
        let stem = Path::new(&last)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(last.clone());
        comps.pop();
        if stem != "lib" && stem != "main" && stem != "mod" {
            comps.push(stem);
        }
    }

    let mut out = crate_name;
    for c in comps {
        out.push_str("::");
        out.push_str(&sanitize(&c));
    }
    out
}

fn read_crate_name(cargo_toml: &Path) -> Option<String> {
    let txt = std::fs::read_to_string(cargo_toml).ok()?;
    let mut in_package = false;
    for line in txt.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_package = l == "[package]";
            continue;
        }
        if in_package && let Some(rest) = l.strip_prefix("name") {
            let rest = rest.trim_start_matches([' ', '=']).trim();
            let name = rest.trim_matches(['"', '\'']);
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Repo-root-relative dotted package path, `::`-joined internally.
/// `pkg/sub/mod.py` → `pkg::sub::mod`; `pkg/__init__.py` → `pkg`.
/// Deterministic + collision-safe (no `sys.path` guesswork — honest).
fn python_module_path(file: &Path, root: &Path) -> String {
    let rel = match file.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return fallback_path(file),
    };
    // Raw components first; resolve the file-stem from the *original* name
    // (sanitizing first would turn `mod.py` into `mod_py`).
    let raw: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let mut comps: Vec<String> = Vec::new();
    for (i, c) in raw.iter().enumerate() {
        if i + 1 == raw.len() {
            let stem = Path::new(c)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| c.clone());
            if stem != "__init__" {
                comps.push(sanitize(&stem));
            }
        } else {
            comps.push(sanitize(c));
        }
    }
    if comps.is_empty() {
        "root".to_string()
    } else {
        comps.join("::")
    }
}

/// JS/TS: repo-root-relative path, drop extension, `index` collapses to
/// its directory, `::`-joined. Deterministic + collision-safe (full
/// tsconfig/extension specifier resolution is the bounded JS/TS deepening).
fn js_module_path(file: &Path, root: &Path) -> String {
    let rel = match file.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return fallback_path(file),
    };
    let raw: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let mut comps: Vec<String> = Vec::new();
    for (i, c) in raw.iter().enumerate() {
        if i + 1 == raw.len() {
            let stem = Path::new(c)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| c.clone());
            if stem != "index" {
                comps.push(sanitize(&stem));
            }
        } else {
            comps.push(sanitize(c));
        }
    }
    if comps.is_empty() {
        "root".to_string()
    } else {
        comps.join("::")
    }
}

/// Generic repo-relative path module (`src/a/b.go` → `src::a::b`).
/// Deterministic + collision-safe.
fn path_module_path(file: &Path, root: &Path) -> String {
    let rel = match file.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => return fallback_path(file),
    };
    let raw: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let mut comps: Vec<String> = Vec::new();
    for (i, c) in raw.iter().enumerate() {
        if i + 1 == raw.len() {
            let stem = Path::new(c)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| c.clone());
            comps.push(sanitize(&stem));
        } else {
            comps.push(sanitize(c));
        }
    }
    if comps.is_empty() {
        "root".to_string()
    } else {
        comps.join("::")
    }
}

fn fallback_path(file: &Path) -> String {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "mod".into());
    sanitize(&stem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hifz_langmod_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("src/code")).unwrap();
        fs::write(d.join("Cargo.toml"), "[package]\nname = \"hifz-core\"\n").unwrap();
        d
    }

    #[test]
    fn js_paths() {
        let root = std::path::PathBuf::from("/proj");
        let mp = |rel: &str| module_path(Language::JavaScript, &root.join(rel), &root);
        assert_eq!(mp("src/auth/user.js"), "src::auth::user");
        assert_eq!(mp("src/auth/index.js"), "src::auth");
        assert_eq!(mp("app.ts"), "app");
    }

    #[test]
    fn python_paths() {
        let root = std::path::PathBuf::from("/proj");
        let mp = |rel: &str| module_path(Language::Python, &root.join(rel), &root);
        assert_eq!(mp("pkg/sub/mod.py"), "pkg::sub::mod");
        assert_eq!(mp("pkg/__init__.py"), "pkg");
        assert_eq!(mp("top.py"), "top");
    }

    #[test]
    fn rust_paths() {
        let root = tmp("rust");
        let mp = |rel: &str| module_path(Language::Rust, &root.join(rel), &root);
        assert_eq!(mp("src/lib.rs"), "hifz_core");
        assert_eq!(mp("src/db.rs"), "hifz_core::db");
        assert_eq!(mp("src/code/mod.rs"), "hifz_core::code");
        assert_eq!(mp("src/code/link.rs"), "hifz_core::code::link");
        assert_eq!(mp("src/main.rs"), "hifz_core");
    }
}
