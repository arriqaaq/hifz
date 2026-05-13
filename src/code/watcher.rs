//! Optional file-watcher: when `HIFZ_CODE_WATCH=1` and
//! `HIFZ_CODE_WATCH_ROOTS` is set (`project=path,project2=path2`), starts a
//! debounced `notify` watcher per pair. Each filesystem event coalesces into
//! a `code::index::index_file` call.
//!
//! Per G10: 500ms debounce. Per-path coalescing collapses bursts. The worker
//! holds a single inflight `index_file` future at a time.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use tokio::sync::mpsc;

use crate::Hifz;

/// Spawn a watcher for `(project, root)`. Returns immediately; the watcher
/// runs on a tokio task. Dropping the returned `WatcherHandle` stops it.
pub fn start_watcher(hifz: Hifz, project: String, root: PathBuf) -> Result<WatcherHandle> {
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();

    let mut debouncer: Debouncer<notify::RecommendedWatcher> = new_debouncer(
        Duration::from_millis(500),
        move |res: DebounceEventResult| match res {
            Ok(events) => {
                for ev in events {
                    let _ = tx.send(ev.path);
                }
            }
            Err(e) => tracing::warn!("watcher error: {e:?}"),
        },
    )
    .context("debouncer init failed")?;
    debouncer
        .watcher()
        .watch(&root, RecursiveMode::Recursive)
        .context("watch failed")?;

    let project_for_task = project.clone();
    let root_for_task = root.clone();
    let handle = tokio::spawn(async move {
        while let Some(path) = rx.recv().await {
            // Only re-index files actually under root (notify can also send
            // events for the watched dir itself).
            if !path.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&root_for_task) else {
                continue;
            };
            tracing::debug!(
                "watcher: change {} (project={})",
                rel.display(),
                project_for_task
            );
            // Use index_repo on the parent root with mtime+hash dedupe — the
            // single touched file is the only one that gets re-chunked.
            let opts = crate::code::index::IndexOpts::default();
            if let Err(e) = crate::code::index::index_repo(
                &hifz.db,
                &hifz.embedder,
                &project_for_task,
                &root_for_task,
                &opts,
            )
            .await
            {
                tracing::warn!("watcher reindex failed: {e}");
            }
        }
    });

    Ok(WatcherHandle {
        _debouncer: Arc::new(debouncer),
        _task: handle,
        project,
        root,
    })
}

#[allow(dead_code)]
pub struct WatcherHandle {
    _debouncer: Arc<Debouncer<notify::RecommendedWatcher>>,
    _task: tokio::task::JoinHandle<()>,
    pub project: String,
    pub root: PathBuf,
}

/// Parse `HIFZ_CODE_WATCH_ROOTS=hifz=/path/to/hifz,docs=/path/to/docs`.
pub fn parse_watch_roots(raw: &str) -> Vec<(String, PathBuf)> {
    raw.split(',')
        .filter_map(|pair| {
            let mut it = pair.splitn(2, '=');
            let project = it.next()?.trim();
            let path = it.next()?.trim();
            if project.is_empty() || path.is_empty() {
                None
            } else {
                Some((project.to_string(), Path::new(path).to_path_buf()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_watch_roots_basic() {
        let pairs = parse_watch_roots("hifz=/tmp/hifz,docs=/tmp/docs");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "hifz");
        assert_eq!(pairs[1].1.display().to_string(), "/tmp/docs");
    }

    #[test]
    fn parse_watch_roots_skips_malformed() {
        let pairs = parse_watch_roots("only_project,proj=/tmp/x,=/empty,proj2=/tmp/y");
        let names: Vec<&str> = pairs.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, vec!["proj", "proj2"]);
    }
}
