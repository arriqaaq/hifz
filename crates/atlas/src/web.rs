//! atlas REST surface — an `axum::Router` the hifz daemon nests at
//! `/api/v1/atlas` behind the `atlas` feature (and the standalone CLI can
//! also serve).
//!
//! Project is **per-request** (`?project=`), not process-fixed. Build
//! endpoints (`/build`, `/code`, `/ingest`, `/extract`, `/cluster`,
//! `/upload`) are **asynchronous**: they spawn a background job and return
//! `{started:true}` immediately; `/status?project=` reports progress. This
//! matches the Glean async-index pattern — a synchronous walk+embed+LLM
//! over a real repo would time out the request/UI.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Multipart, Query, State},
    routing::{get, post},
};
use dashmap::DashMap;
use kernel::db::Db;
use kernel::embed::Embedder;
use serde::Deserialize;
use serde_json::{Value, json};
use surrealdb::Surreal;

use crate::store::Store;

#[derive(Clone, Default)]
pub struct JobState {
    pub running: bool,
    pub step: String,
    pub last_report: Option<Value>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct AtlasState {
    pub db: Surreal<Db>,
    pub embedder: Arc<Embedder>,
    /// Per-project background build status.
    pub jobs: Arc<DashMap<String, JobState>>,
}

#[derive(Deserialize)]
struct Scoped {
    #[serde(default)]
    project: Option<String>,
}

#[derive(Deserialize)]
struct PathReq {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    git: Option<String>,
    #[serde(default)]
    docs: Option<String>,
    #[serde(default)]
    project: Option<String>,
}

#[derive(Deserialize)]
struct Q {
    q: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    project: Option<String>,
}

fn proj(p: Option<String>) -> String {
    p.filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "default".into())
}
fn store(s: &AtlasState, p: Option<String>) -> Store {
    Store::new(s.db.clone(), proj(p))
}
fn err(e: anyhow::Error) -> Json<Value> {
    Json(json!({ "error": e.to_string() }))
}

pub fn router(state: AtlasState) -> Router {
    Router::new()
        .route("/build", post(build))
        .route("/code", post(code))
        .route("/ingest", post(ingest))
        .route("/extract", post(extract))
        .route("/cluster", post(cluster))
        .route("/upload", post(upload))
        .route("/status", get(status))
        .route("/insights", get(insights))
        .route("/graph", get(graph))
        .route("/query", get(query))
        .with_state(state)
}

// --- async job machinery ------------------------------------------------
// DashMap guards are never held across an `.await` (only sync helpers).

fn job_set(jobs: &Arc<DashMap<String, JobState>>, project: &str, f: impl FnOnce(&mut JobState)) {
    let mut e = jobs.entry(project.to_string()).or_default();
    f(e.value_mut());
}
fn job_step(jobs: &Arc<DashMap<String, JobState>>, project: &str, step: &str) {
    job_set(jobs, project, |j| j.step = step.to_string());
}
fn job_done(jobs: &Arc<DashMap<String, JobState>>, project: &str, report: anyhow::Result<Value>) {
    job_set(jobs, project, |j| {
        j.running = false;
        j.step = "idle".into();
        match report {
            Ok(v) => {
                j.last_report = Some(v);
                j.error = None;
            }
            Err(e) => j.error = Some(e.to_string()),
        }
    });
}

/// Spawn `run` on a background task (owned `Store` + `Arc<Embedder>` so the
/// future is `'static + Send`). Returns immediately. One job per project.
fn spawn_job<F, Fut>(s: &AtlasState, project: String, run: F) -> Json<Value>
where
    F: FnOnce(Store, Arc<Embedder>, Arc<DashMap<String, JobState>>, String) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    if s.jobs.get(&project).map(|j| j.running).unwrap_or(false) {
        return Json(json!({ "error": "build already running", "project": project }));
    }
    job_set(&s.jobs, &project, |j| {
        *j = JobState {
            running: true,
            step: "starting".into(),
            last_report: None,
            error: None,
        }
    });
    let st = Store::new(s.db.clone(), project.clone());
    let emb = s.embedder.clone();
    let jobs = s.jobs.clone();
    let p = project.clone();
    tokio::spawn(run(st, emb, jobs, p));
    Json(json!({ "started": true, "project": project }))
}

async fn status(State(s): State<AtlasState>, Query(sc): Query<Scoped>) -> Json<Value> {
    let p = proj(sc.project);
    let j = s.jobs.get(&p).map(|j| j.clone()).unwrap_or_default();
    Json(json!({
        "project": p, "running": j.running, "step": j.step,
        "last_report": j.last_report, "error": j.error,
    }))
}

// --- git clone / paths --------------------------------------------------

fn home_base() -> PathBuf {
    if let Ok(db) = std::env::var("HIFZ_DB") {
        if let Some(par) = std::path::Path::new(&db).parent() {
            if !par.as_os_str().is_empty() {
                return par.to_path_buf();
            }
        }
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".hifz")
}
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `git clone --depth 1` (or `pull --ff-only` if present) the URL into a
/// local dir; returns it. Resolves `git` via the `which` dep.
fn git_sync(url: &str) -> anyhow::Result<PathBuf> {
    let git = which::which("git").map_err(|_| anyhow::anyhow!("`git` not found on PATH"))?;
    let slug = sanitize(
        &url.trim_end_matches(".git")
            .rsplit('/')
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("_"),
    );
    let dir = home_base().join("atlas-repos").join(slug);
    if dir.join(".git").is_dir() {
        let ok = Command::new(&git)
            .arg("-C")
            .arg(&dir)
            .args(["pull", "--ff-only"])
            .status()?
            .success();
        if !ok {
            anyhow::bail!("git pull failed for {url}");
        }
    } else {
        std::fs::create_dir_all(dir.parent().unwrap_or(&dir))?;
        let ok = Command::new(&git)
            .args(["clone", "--depth", "1", url])
            .arg(&dir)
            .status()?
            .success();
        if !ok {
            anyhow::bail!("git clone failed for {url}");
        }
    }
    Ok(dir)
}

// --- build handlers (spawn jobs) ---------------------------------------

async fn run_build(
    st: &Store,
    emb: &Embedder,
    jobs: &Arc<DashMap<String, JobState>>,
    p: &str,
    r: PathReq,
) -> anyhow::Result<Value> {
    let mut out = serde_json::Map::new();
    let code_root: Option<PathBuf> = if let Some(g) = r.git.as_deref().filter(|s| !s.is_empty()) {
        job_step(jobs, p, "git clone");
        Some(git_sync(g)?)
    } else {
        r.path
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    if let Some(root) = &code_root {
        job_step(jobs, p, "index code");
        out.insert(
            "code".into(),
            json!(crate::code::project_code_graph(st, emb, root).await?),
        );
    }
    if let Some(docs) = r.docs.as_deref().filter(|s| !s.is_empty()) {
        job_step(jobs, p, "ingest docs");
        out.insert(
            "ingest".into(),
            json!(crate::ingest::ingest_path(st, emb, std::path::Path::new(docs)).await?),
        );
    }
    job_step(jobs, p, "extract concepts");
    let backend = crate::llm::LlmBackend::from_env();
    out.insert(
        "extract".into(),
        json!(crate::extract::extract_concepts(st, emb, backend.as_ref()).await?),
    );
    job_step(jobs, p, "cluster");
    out.insert("cluster".into(), json!(crate::cluster::cluster(st).await?));
    Ok(Value::Object(out))
}

async fn build(State(s): State<AtlasState>, Json(r): Json<PathReq>) -> Json<Value> {
    let project = proj(r.project.clone());
    spawn_job(&s, project, move |st, emb, jobs, p| async move {
        let res = run_build(&st, &emb, &jobs, &p, r).await;
        job_done(&jobs, &p, res);
    })
}

async fn code(State(s): State<AtlasState>, Json(r): Json<PathReq>) -> Json<Value> {
    let project = proj(r.project.clone());
    spawn_job(&s, project, move |st, emb, jobs, p| async move {
        let res = async {
            let root = if let Some(g) = r.git.as_deref().filter(|s| !s.is_empty()) {
                job_step(&jobs, &p, "git clone");
                git_sync(g)?
            } else {
                PathBuf::from(r.path.clone().unwrap_or_default())
            };
            job_step(&jobs, &p, "index code");
            Ok::<_, anyhow::Error>(json!(
                crate::code::project_code_graph(&st, &emb, &root).await?
            ))
        }
        .await;
        job_done(&jobs, &p, res);
    })
}

async fn ingest(State(s): State<AtlasState>, Json(r): Json<PathReq>) -> Json<Value> {
    let project = proj(r.project.clone());
    spawn_job(&s, project, move |st, emb, jobs, p| async move {
        let path = r.path.clone().unwrap_or_default();
        job_step(&jobs, &p, "ingest docs");
        let res = crate::ingest::ingest_path(&st, &emb, std::path::Path::new(&path))
            .await
            .map(|r| json!(r));
        job_done(&jobs, &p, res);
    })
}

async fn extract(State(s): State<AtlasState>, Query(sc): Query<Scoped>) -> Json<Value> {
    let project = proj(sc.project);
    spawn_job(&s, project, move |st, emb, jobs, p| async move {
        job_step(&jobs, &p, "extract concepts");
        let backend = crate::llm::LlmBackend::from_env();
        let res = crate::extract::extract_concepts(&st, &emb, backend.as_ref())
            .await
            .map(|r| json!(r));
        job_done(&jobs, &p, res);
    })
}

async fn cluster(State(s): State<AtlasState>, Query(sc): Query<Scoped>) -> Json<Value> {
    let project = proj(sc.project);
    spawn_job(&s, project, move |st, _emb, jobs, p| async move {
        job_step(&jobs, &p, "cluster");
        let res = crate::cluster::cluster(&st).await.map(|r| json!(r));
        job_done(&jobs, &p, res);
    })
}

async fn upload(
    State(s): State<AtlasState>,
    Query(sc): Query<Scoped>,
    mut mp: Multipart,
) -> Json<Value> {
    let project = proj(sc.project);
    let dir = home_base().join("atlas-uploads").join(sanitize(&project));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return err(e.into());
    }
    // best-effort prune of uploads older than 7 days
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let cutoff =
            std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(7 * 24 * 3600));
        for ent in rd.flatten() {
            if let (Some(c), Ok(m)) = (cutoff, ent.metadata()) {
                if m.modified().map(|t| t < c).unwrap_or(false) {
                    let _ = std::fs::remove_file(ent.path());
                }
            }
        }
    }
    const ALLOWED: &[&str] = &["pdf", "md", "markdown", "mdx", "txt", "rst"];
    let mut n = 0usize;
    loop {
        let field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return Json(json!({ "error": format!("multipart: {e}") })),
        };
        let fname = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("upload{n}"));
        let ext = std::path::Path::new(&fname)
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !ALLOWED.contains(&ext.as_str()) {
            continue;
        }
        let bytes = match field.bytes().await {
            Ok(b) => b,
            Err(e) => return Json(json!({ "error": format!("read {fname}: {e}") })),
        };
        if bytes.len() > 25 * 1024 * 1024 {
            return Json(json!({ "error": format!("{fname} exceeds 25 MiB") }));
        }
        if std::fs::write(dir.join(sanitize(&fname)), &bytes).is_ok() {
            n += 1;
        }
    }
    if n == 0 {
        return Json(json!({
            "error": "no accepted files (pdf|md|markdown|mdx|txt|rst, ≤25 MiB each)"
        }));
    }
    let dir2 = dir.clone();
    spawn_job(&s, project, move |st, emb, jobs, p| async move {
        job_step(&jobs, &p, "ingest uploads");
        let res = crate::ingest::ingest_path(&st, &emb, &dir2)
            .await
            .map(|r| json!(r));
        job_done(&jobs, &p, res);
    })
}

// --- read handlers ------------------------------------------------------

async fn insights(State(s): State<AtlasState>, Query(sc): Query<Scoped>) -> Json<Value> {
    match crate::analyze::analyze(&store(&s, sc.project)).await {
        Ok(i) => Json(json!(i)),
        Err(e) => err(e),
    }
}

async fn query(State(s): State<AtlasState>, Query(q): Query<Q>) -> Json<Value> {
    match crate::query::query(&store(&s, q.project.clone()), &q.q, q.limit.unwrap_or(20)).await {
        Ok(h) => Json(json!({ "hits": h })),
        Err(e) => err(e),
    }
}

/// Node+edge dump for the UI (Cytoscape-friendly).
async fn graph(State(s): State<AtlasState>, Query(sc): Query<Scoped>) -> Json<Value> {
    let p = proj(sc.project);
    let nodes =
        s.db.query("SELECT id, kind, label, cluster FROM atlas_node WHERE project=$p LIMIT 4000")
            .bind(("p", p.clone()))
            .await
            .and_then(|mut r| r.take::<Vec<Value>>(0))
            .unwrap_or_default();
    let edges =
        s.db.query(
            "SELECT in, out, relation, resolution, score FROM atlas_edge \
             WHERE in IN (SELECT VALUE id FROM atlas_node WHERE project=$p) LIMIT 8000",
        )
        .bind(("p", p))
        .await
        .and_then(|mut r| r.take::<Vec<Value>>(0))
        .unwrap_or_default();
    Json(json!({ "nodes": nodes, "edges": edges }))
}
