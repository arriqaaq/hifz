//! Git-hook adapter — out-of-band commit detection.
//!
//! Today hifz only learns about a commit when Claude runs `git commit` via the
//! Bash tool (the Claude Code `PostToolUse` hook). A commit made in a plain
//! terminal, a `git pull` that merges a PR, or a rebase is invisible. This
//! adapter installs real `.git/hooks/{post-commit,post-merge,post-rewrite}`
//! that emit hifz's existing `commit_made` observation — so memory grounding
//! works regardless of how/where the commit was made.
//!
//! It is a thin client: it POSTs the same `/api/v1/agent/observe` contract the
//! `.mjs` adapter already uses. Because the server-side watermark write is
//! idempotent by algebra (last-write-wins timestamp + absorbing `forget_after`
//! un-set), a commit seen by *both* this hook and the Claude path just sets the
//! same timestamp twice — no dedup/identity machinery is needed here.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;

const MARKER_START: &str = "# >>> hifz hook >>>";
const MARKER_END: &str = "# <<< hifz hook <<<";
const HOOKS: &[&str] = &["post-commit", "post-merge", "post-rewrite"];

#[derive(Subcommand, Debug)]
pub enum HookAction {
    /// Install the hooks into the current repo (idempotent; coexists with existing hooks).
    Install,
    /// Remove hifz's hook blocks from the current repo.
    Uninstall,
    /// Show whether each hook is installed in the current repo.
    Status,
    /// Diagnose: PATH, server reachability, hooks present/executable.
    Doctor,
    /// Internal: invoked by the installed hooks. Ingests the given commit SHAs.
    Ingest {
        /// Which hook fired (post-commit | post-merge | post-rewrite).
        #[arg(long)]
        event: String,
        /// Space-separated commit SHAs, captured synchronously by the hook.
        #[arg(long, default_value = "")]
        shas: String,
    },
}

pub async fn run(action: HookAction) -> Result<()> {
    match action {
        HookAction::Install => install(),
        HookAction::Uninstall => uninstall(),
        HookAction::Status => status(),
        HookAction::Doctor => doctor().await,
        HookAction::Ingest { event, shas } => ingest(&event, &shas).await,
    }
}

// git helpers

fn git(args: &[&str], cwd: &Path) -> Result<String> {
    let out = Proc::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn cwd() -> Result<PathBuf> {
    std::env::current_dir().context("resolving current dir")
}

fn repo_root(at: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(&["rev-parse", "--show-toplevel"], at)?))
}

/// Resolve the hooks directory, honoring `core.hooksPath` (Husky/pre-commit).
fn hooks_dir(root: &Path) -> Result<PathBuf> {
    if let Ok(p) = git(&["config", "--get", "core.hooksPath"], root)
        && !p.is_empty()
    {
        let pb = if Path::new(&p).is_absolute() {
            PathBuf::from(&p)
        } else {
            root.join(&p)
        };
        return Ok(pb);
    }
    let gp = git(&["rev-parse", "--git-path", "hooks"], root)?;
    let pb = if Path::new(&gp).is_absolute() {
        PathBuf::from(gp)
    } else {
        root.join(gp)
    };
    Ok(pb)
}

// hook script bodies

/// The hifz block for a given hook. Captures the SHA list *synchronously*
/// (inside git, before detaching) — then backgrounds the
/// ingest so `git` returns immediately. Never blocks or fails git.
fn hook_block(event: &str) -> String {
    let capture = match event {
        // New commit is HEAD.
        "post-commit" => "SHAS=\"$(git rev-parse HEAD 2>/dev/null)\"".to_string(),
        // A merge/FF-pull moved HEAD over a range; ORIG_HEAD..HEAD (fallback
        // HEAD@{1}..HEAD), else just the merge commit.
        "post-merge" => "BASE=\"$(git rev-parse ORIG_HEAD 2>/dev/null || git rev-parse 'HEAD@{1}' 2>/dev/null)\"; \
             if [ -n \"$BASE\" ]; then SHAS=\"$(git rev-list \"$BASE..HEAD\" 2>/dev/null | tr '\\n' ' ')\"; fi; \
             [ -z \"$SHAS\" ] && SHAS=\"$(git rev-parse HEAD 2>/dev/null)\""
            .to_string(),
        // post-rewrite (amend/rebase): old/new SHA pairs on stdin; take new.
        "post-rewrite" => "SHAS=\"$(while read -r _old new _rest; do printf '%s ' \"$new\"; done)\""
            .to_string(),
        _ => "SHAS=\"$(git rev-parse HEAD 2>/dev/null)\"".to_string(),
    };
    format!(
        "{MARKER_START}\n\
         {capture}\n\
         if command -v hifz >/dev/null 2>&1 && [ -n \"${{SHAS}}\" ]; then \
           ( nohup hifz hook ingest --event {event} --shas \"$SHAS\" \
             >>\"${{HOME}}/.hifz/hook.log\" 2>&1 & ) >/dev/null 2>&1 || true; \
         fi\n\
         {MARKER_END}\n"
    )
}

fn strip_block(body: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in body.lines() {
        if line.trim() == MARKER_START {
            skip = true;
            continue;
        }
        if line.trim() == MARKER_END {
            skip = false;
            continue;
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// actions

fn install() -> Result<()> {
    let at = cwd()?;
    let root = repo_root(&at)?;
    let dir = hooks_dir(&root)?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating hooks dir {}", dir.display()))?;
    // Ensure ~/.hifz exists for the hook log.
    if let Some(home) = dirs_home() {
        let _ = std::fs::create_dir_all(home.join(".hifz"));
    }

    for hook in HOOKS {
        let path = dir.join(hook);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let cleaned = strip_block(&existing);
        let mut body = if cleaned.trim().is_empty() {
            "#!/bin/sh\n".to_string()
        } else if cleaned.starts_with("#!") {
            cleaned
        } else {
            format!("#!/bin/sh\n{cleaned}")
        };
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&hook_block(hook));
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        set_executable(&path)?;
        println!("installed: {}", path.display());
    }
    println!("hifz git hooks installed in {}", dir.display());
    Ok(())
}

fn uninstall() -> Result<()> {
    let at = cwd()?;
    let root = repo_root(&at)?;
    let dir = hooks_dir(&root)?;
    for hook in HOOKS {
        let path = dir.join(hook);
        let Ok(existing) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !existing.contains(MARKER_START) {
            continue;
        }
        let cleaned = strip_block(&existing);
        if cleaned.trim().is_empty() || cleaned.trim() == "#!/bin/sh" {
            std::fs::remove_file(&path).ok();
            println!("removed: {}", path.display());
        } else {
            std::fs::write(&path, cleaned)
                .with_context(|| format!("rewriting {}", path.display()))?;
            println!("cleaned: {}", path.display());
        }
    }
    println!("hifz git hooks uninstalled");
    Ok(())
}

fn status() -> Result<()> {
    let at = cwd()?;
    let root = repo_root(&at)?;
    let dir = hooks_dir(&root)?;
    println!("repo: {}", root.display());
    println!("hooks dir: {}", dir.display());
    for hook in HOOKS {
        let path = dir.join(hook);
        let installed = std::fs::read_to_string(&path)
            .map(|b| b.contains(MARKER_START))
            .unwrap_or(false);
        println!(
            "  {hook}: {}",
            if installed {
                "installed"
            } else {
                "not installed"
            }
        );
    }
    Ok(())
}

async fn doctor() -> Result<()> {
    status()?;
    // hifz on PATH?
    let on_path = Proc::new("sh")
        .args(["-c", "command -v hifz"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    println!(
        "hifz on PATH: {}",
        if on_path {
            "yes"
        } else {
            "NO — hooks will silently no-op until fixed"
        }
    );
    // Server reachable?
    let url = rest_url();
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();
    match client.get(format!("{url}/api/v1/health")).send().await {
        Ok(r) if r.status().is_success() => println!("server: reachable at {url}"),
        Ok(r) => println!("server: {url} returned {}", r.status()),
        Err(e) => println!("server: UNREACHABLE at {url} ({e})"),
    }
    let log = dirs_home().map(|h| h.join(".hifz/hook.log"));
    if let Some(l) = log {
        println!("hook log: {}", l.display());
    }
    Ok(())
}

async fn ingest(event: &str, shas: &str) -> Result<()> {
    let at = cwd()?;
    let root = repo_root(&at)?;
    let project = root.to_string_lossy().to_string();
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"], &root).unwrap_or_default();
    let session_id = format!("git-hook:{:x}", seahash(project.as_bytes()));
    let url = rest_url();
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    for sha in shas.split_whitespace() {
        if sha.len() < 7 {
            continue;
        }
        if let Err(e) = ingest_one(
            &client,
            &url,
            &root,
            &project,
            &branch,
            &session_id,
            event,
            sha,
        )
        .await
        {
            // Never fail git / never bubble — just log to stderr (→ hook.log).
            eprintln!("[hifz hook] ingest {sha} failed: {e}");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn ingest_one(
    client: &reqwest::Client,
    url: &str,
    root: &Path,
    project: &str,
    branch: &str,
    session_id: &str,
    event: &str,
    sha: &str,
) -> Result<()> {
    let subject = git(&["log", "-1", "--format=%s", sha], root).unwrap_or_default();
    let full_message = git(&["log", "-1", "--format=%B", sha], root).unwrap_or_default();
    // Author gate (computed *in-repo*, where local identity is correct — a
    // server-side `git config` check would run in the daemon's cwd). The
    // watermark/persistence still fires for any author; only the semantic
    // `commits_for` linker is gated server-side on this flag. Unknown local
    // identity → treat as local (fail-open, backward-compatible).
    let author_email = git(&["log", "-1", "--format=%ae", sha], root).unwrap_or_default();
    let local_email = git(&["config", "user.email"], root).unwrap_or_default();
    let authored_locally =
        local_email.is_empty() || author_email.is_empty() || author_email == local_email;
    // `--first-parent` keeps merge commits to the mainline file set (a plain
    // diff-tree is empty on a merge); harmless on normal commits.
    let name_status = git(
        &[
            "diff-tree",
            "--no-commit-id",
            "-r",
            "--first-parent",
            "--name-status",
            sha,
        ],
        root,
    )
    .unwrap_or_default();

    let mut files = Vec::new();
    let mut file_status = Vec::new();
    for line in name_status.lines().filter(|l| !l.is_empty()) {
        let parts: Vec<&str> = line.split('\t').filter(|s| !s.is_empty()).collect();
        if parts.len() < 2 {
            continue;
        }
        let code = parts[0].chars().next().unwrap_or('M');
        let path = parts[parts.len() - 1].to_string();
        let st = match code {
            'A' => "A",
            'D' => "D",
            _ => "M", // M / R / C → modify
        };
        file_status.push(serde_json::json!({ "path": path, "status": st }));
        files.push(path);
    }

    // Revert detection (mirrors the .mjs adapter).
    let first_line = full_message.lines().next().unwrap_or("");
    let lc = first_line.to_lowercase();
    let reverts_sha = full_message
        .lines()
        .find_map(|l| l.trim().strip_prefix("This reverts commit "))
        .map(|s| s.trim_end_matches('.').to_string());
    let is_revert = first_line.starts_with("Revert \"")
        || lc.starts_with("revert")
        || lc.starts_with("rollback")
        || lc.starts_with("undo ")
        || reverts_sha.is_some();

    let keywords: Vec<String> = subject
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_string())
        .collect();

    let body = serde_json::json!({
        "hookType": "post_commit",
        "sessionId": session_id,
        "project": project,
        "cwd": project,
        "obs_type": "commit_made",
        "source": "git-hook",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "title": format!("commit: {branch}: {subject}"),
        "facts": [format!("sha:{sha}"), format!("branch:{branch}")],
        "keywords": keywords,
        "files": files,
        "data": { "tool_name": "git-hook", "tool_input": { "event": event } },
        "metadata": {
            "sha": sha,
            "branch": branch,
            "message": full_message,
            "files": files,
            "file_status": file_status,
            "is_revert": is_revert,
            "reverts_sha": reverts_sha,
            "author_email": author_email,
            "authored_locally": authored_locally,
        },
        "importance": 8,
    });

    let resp = client
        .post(format!("{url}/api/v1/agent/observe"))
        .json(&body)
        .send()
        .await
        .context("POST /api/v1/agent/observe")?;
    if !resp.status().is_success() {
        return Err(anyhow!("server returned {}", resp.status()));
    }
    Ok(())
}

// small utils

fn rest_url() -> String {
    std::env::var("HIFZ_URL").unwrap_or_else(|_| "http://127.0.0.1:3111".to_string())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Tiny stable hash (FNV-1a) for a per-repo synthetic session id. Not security.
fn seahash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
