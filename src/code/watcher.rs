//! Live code watcher — keeps the code index in sync with on-disk edits in
//! real time, so an MCP search never misses a function that exists on disk.
//!
//! One watcher per `(project, root)`, normally auto-started for every indexed
//! project (see `Hifz::start_watch` / boot discovery in `main.rs`). Filesystem
//! events are filtered (`PathFilter` — skips `target/`, `.git/`, gitignored and
//! non-source paths so a `cargo build` can't thrash the index), debounced
//! ~200ms to coalesce an editor's multi-event save and survive bulk events like
//! `git checkout`, then drained as a batch by the worker.
//!
//! Per batch the worker, for each path: re-chunks + embeds it (`index::index_file`,
//! fresh `code_chunk`) and re-parses it into a task-local `FileGraphCache`; for a
//! vanished path it reconciles the delete. It then runs ONE synchronous
//! `intel::resolve_and_persist` over the cached graphs. After a batch,
//! chunks **and** symbols **and** edges are consistent with disk — there is no
//! deferred/lagging pass. `resolve_project` is pure and cheap; the cost the
//! cache avoids is re-parsing the whole repo (the stack-graphs model).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use tokio::sync::mpsc;

use crate::Hifz;
use crate::code::walker::PathFilter;

/// Debounce window: coalesces an editor's write-temp + rename + chmod into one
/// batch and absorbs bulk events. Not an eventual-consistency delay — the index
/// is fully consistent once the batch is processed.
const DEBOUNCE_MS: u64 = 200;

/// Spawn a watcher for `(project, root)`. Returns immediately; the watcher runs
/// on a tokio task that owns its debouncer and an in-memory `FileGraphCache`.
/// Dropping (or `stop`-ing) the returned `WatcherHandle` aborts the task, which
/// drops the debouncer and ends the OS watch.
pub fn start_watcher(hifz: Hifz, project: String, root: PathBuf) -> Result<WatcherHandle> {
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();

    // Filter events on notify's thread so churn (target/, gitignored, non-source)
    // never reaches the worker. `is_indexable` is path-based, so it also passes
    // deletes of source files through (the file is gone, but the path still ends
    // in a known extension).
    let filter = PathFilter::new(&root);
    let mut debouncer = new_debouncer(
        Duration::from_millis(DEBOUNCE_MS),
        move |res: DebounceEventResult| match res {
            Ok(events) => {
                for ev in events {
                    if filter.is_indexable(&ev.path) {
                        let _ = tx.send(ev.path);
                    }
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

    let project_task = project.clone();
    let root_task = root.clone();
    let task = tokio::spawn(async move {
        // Move the debouncer into the task so it lives exactly as long as the
        // task; on abort it drops and the OS watch stops.
        let _debouncer = debouncer;

        // Warm the FileGraph cache once (parse the repo off the async pool).
        let mut cache = tokio::task::spawn_blocking({
            let r = root_task.clone();
            move || crate::code::intel::build_cache(&r)
        })
        .await
        .unwrap_or_default();
        tracing::info!(
            "code watcher ready: project={project_task} root={} cached_files={}",
            root_task.display(),
            cache.len()
        );

        while let Some(first) = rx.recv().await {
            // Coalesce the current burst: drain everything already queued.
            let mut paths: HashSet<PathBuf> = HashSet::new();
            paths.insert(first);
            while let Ok(p) = rx.try_recv() {
                paths.insert(p);
            }

            let mut changed: HashSet<String> = HashSet::new();
            let mut any_delete = false;

            for path in paths {
                let rel = match path.strip_prefix(&root_task) {
                    Ok(p) => p.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };
                if path.exists() {
                    // Created/modified → re-chunk + embed, then re-parse into cache.
                    if let Err(e) = crate::code::index::index_file(
                        &hifz.db,
                        &hifz.embedder,
                        &project_task,
                        &root_task,
                        &path,
                    )
                    .await
                    {
                        tracing::warn!("watcher index_file failed for {rel}: {e}");
                        continue;
                    }
                    match crate::code::intel::build_file_graph(&path, &rel, &root_task) {
                        Some(cf) => {
                            cache.insert(rel.clone(), cf);
                            changed.insert(rel);
                        }
                        // No longer a parseable source file — drop from the graph set.
                        None => {
                            cache.remove(&rel);
                        }
                    }
                } else {
                    // Deleted / renamed-away → purge its chunks/symbols/edges.
                    let _ = crate::code::gc::reconcile_deleted_file(&hifz.db, &project_task, &rel)
                        .await;
                    cache.remove(&rel);
                    any_delete = true;
                }
            }

            // One synchronous project-wide resolve+persist for the batch. Symbol
            // UPSERT is scoped to changed files; rename/stale reconcile and the
            // derived-edge rebuild always run project-wide (so a delete re-resolves
            // cross-file callers correctly).
            if (!changed.is_empty() || any_delete)
                && let Err(e) = crate::code::intel::resolve_and_persist(
                    &hifz.db,
                    &project_task,
                    &cache,
                    Some(&changed),
                )
                .await
            {
                tracing::warn!("watcher resolve_and_persist failed (project={project_task}): {e}");
            }
        }
    });

    Ok(WatcherHandle {
        abort: task.abort_handle(),
        project,
        root,
    })
}

/// Registry entry for an active watcher. Holds only the task's `AbortHandle`
/// (`Send + Sync`) so it can live in a shared `DashMap` on `Hifz`; the debouncer
/// itself is owned by the task. Dropping this aborts the task and stops the watch.
pub struct WatcherHandle {
    abort: tokio::task::AbortHandle,
    pub project: String,
    pub root: PathBuf,
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// Parse `HIFZ_CODE_WATCH_ROOTS=hifz=/path/to/hifz,docs=/path/to/docs`
/// (env-var override; auto-discovery from `code_file` is the default).
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
