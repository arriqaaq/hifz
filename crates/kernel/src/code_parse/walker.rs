//! Gitignore-honest file walking with binary + size guards.
//!
//! Built on `ignore::WalkBuilder` (the engine ripgrep uses). Honors the root
//! `.gitignore`, parent gitignores, `.git/info/exclude`, and global excludes.
//! Skips files whose first 512 bytes contain a NUL (cheap binary heuristic)
//! and files larger than `WalkOpts.max_file_bytes` (default 2 MiB).
//!
//! Only files whose extension maps to a known `Language` (see `lang.rs`) are
//! emitted — anything else (assets, lockfiles, etc) is invisible to the
//! indexer at v1.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use ignore::WalkBuilder;

use crate::code_parse::lang::is_supported_extension;

/// One walked file ready for indexing. `mtime_ns` and `size_bytes` are used by
/// `code_file` rows to short-circuit unchanged files on re-index.
#[derive(Debug, Clone)]
pub struct WalkedFile {
    pub abs: PathBuf,
    /// Repo-relative POSIX path (forward slashes).
    pub rel: String,
    pub size_bytes: u64,
    pub mtime_ns: i128,
}

#[derive(Debug, Clone)]
pub struct WalkOpts {
    pub follow_symlinks: bool,
    pub max_file_bytes: u64,
    /// Treat hidden (dot) files as candidates? Default false (skip them).
    pub include_hidden: bool,
}

impl Default for WalkOpts {
    fn default() -> Self {
        Self {
            follow_symlinks: false,
            max_file_bytes: 2 * 1024 * 1024,
            include_hidden: false,
        }
    }
}

pub fn walk(root: &Path, opts: &WalkOpts) -> Result<Vec<WalkedFile>> {
    let walker = WalkBuilder::new(root)
        .follow_links(opts.follow_symlinks)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .hidden(!opts.include_hidden)
        .build();

    let mut out: Vec<WalkedFile> = Vec::new();
    for ent in walker {
        let ent = match ent {
            Ok(e) => e,
            Err(e) => {
                tracing::trace!("walker entry error: {e}");
                continue;
            }
        };
        if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = ent.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !is_supported_extension(ext) {
            continue;
        }
        let meta = match path.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > opts.max_file_bytes {
            tracing::debug!(
                "skipping {}: {} bytes > {} cap",
                path.display(),
                meta.len(),
                opts.max_file_bytes
            );
            continue;
        }
        if has_nul_in_first_512(path) {
            continue;
        }

        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);

        let rel = match path.strip_prefix(root) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if rel.is_empty() {
            continue;
        }

        out.push(WalkedFile {
            abs: path.to_path_buf(),
            rel,
            size_bytes: meta.len(),
            mtime_ns,
        });
    }
    Ok(out)
}

fn has_nul_in_first_512(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else {
        return true;
    };
    let mut buf = [0u8; 512];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return true,
    };
    buf[..n].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hifz_walker_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn walks_supported_files_only() {
        let root = tmp_dir("supported");
        fs::write(root.join("a.rs"), "fn main() {}").unwrap();
        fs::write(root.join("b.txt"), "ignored").unwrap();
        fs::write(root.join("c.py"), "x = 1").unwrap();

        let files = walk(&root, &WalkOpts::default()).unwrap();
        let exts: Vec<_> = files.iter().map(|f| f.rel.clone()).collect();
        assert!(exts.contains(&"a.rs".to_string()));
        assert!(exts.contains(&"c.py".to_string()));
        assert!(!exts.iter().any(|e| e.ends_with("b.txt")));
    }

    #[test]
    fn honors_gitignore() {
        let root = tmp_dir("gitignore");
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/build.rs"), "fn main() {}").unwrap();
        fs::write(root.join("ok.rs"), "fn main() {}").unwrap();
        // Initialize as a git repo so ignore picks up rules
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();

        let files = walk(&root, &WalkOpts::default()).unwrap();
        let rels: Vec<_> = files.iter().map(|f| f.rel.clone()).collect();
        assert!(rels.contains(&"ok.rs".to_string()));
        assert!(
            !rels.iter().any(|r| r.starts_with("target/")),
            "target/ should be excluded by .gitignore: got {rels:?}"
        );
    }

    #[test]
    fn skips_oversized_files() {
        let root = tmp_dir("size");
        let big: String = "x".repeat(10_000);
        fs::write(root.join("big.rs"), &big).unwrap();
        fs::write(root.join("small.rs"), "fn x() {}").unwrap();

        let opts = WalkOpts {
            max_file_bytes: 1_000,
            ..WalkOpts::default()
        };
        let files = walk(&root, &opts).unwrap();
        let rels: Vec<_> = files.iter().map(|f| f.rel.clone()).collect();
        assert!(rels.contains(&"small.rs".to_string()));
        assert!(!rels.contains(&"big.rs".to_string()));
    }

    #[test]
    fn skips_binary_files() {
        let root = tmp_dir("binary");
        // .rs file with NUL bytes in first 512 — skipped by heuristic.
        let mut blob = vec![b'a', b'b'];
        blob.push(0);
        blob.extend(b"more bytes after nul");
        fs::write(root.join("binlike.rs"), &blob).unwrap();
        fs::write(root.join("ok.rs"), "fn main() {}").unwrap();

        let files = walk(&root, &WalkOpts::default()).unwrap();
        let rels: Vec<_> = files.iter().map(|f| f.rel.clone()).collect();
        assert!(rels.contains(&"ok.rs".to_string()));
        assert!(!rels.contains(&"binlike.rs".to_string()));
    }
}
