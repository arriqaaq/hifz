//! atlas CLI — the primary surface. Subcommands run the pipeline against
//! the shared SurrealKV instance (same store as hifz, isolated tables).

use std::sync::Arc;

use anyhow::Result;
use atlas::store::{Store, init_atlas_schema};
use clap::{Parser, Subcommand};
use kernel::embed::Embedder;

#[derive(Parser)]
#[command(name = "atlas", about = "corpus knowledge graph on hifz")]
struct Cli {
    /// SurrealKV data dir (shared with hifz). Defaults to $HIFZ_DB or ~/.hifz/data.
    #[arg(long, env = "HIFZ_DB")]
    db: Option<String>,
    /// Project scope.
    #[arg(long, default_value = "default")]
    project: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ingest PDFs/markdown/txt under a path.
    Ingest { path: String },
    /// Project a repo's code graph (scope-qualified, 3-state resolution).
    Code { root: String },
    /// LLM concept extraction over ingested docs (no-LLM fallback if no backend).
    Extract,
    /// Modularity clustering + >25% re-split.
    Cluster,
    /// Hub / surprising / isolated analytics (JSON).
    Insights,
    /// Hybrid text query over the corpus graph.
    Query {
        text: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Serve the atlas REST API standalone.
    Serve {
        #[arg(long, default_value_t = 3140)]
        port: u16,
    },
}

fn default_db() -> String {
    std::env::var("HIFZ_DB").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{home}/.hifz/data")
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "atlas=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let path = cli.db.clone().unwrap_or_else(default_db);
    let db = kernel::db::connect(&path).await?;
    let embedder = Embedder::new()?;
    init_atlas_schema(&db, embedder.dimension()).await?;
    let store = Store::new(db.clone(), cli.project.clone());

    match cli.cmd {
        Cmd::Ingest { path } => {
            let r =
                atlas::ingest::ingest_path(&store, &embedder, std::path::Path::new(&path)).await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        Cmd::Code { root } => {
            let r = atlas::code::project_code_graph(&store, &embedder, std::path::Path::new(&root))
                .await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        Cmd::Extract => {
            let backend = atlas::llm::LlmBackend::from_env();
            if backend.is_none() {
                eprintln!("no LLM backend (set OLLAMA_URL or ATLAS_LLM) — using fallback");
            }
            let r = atlas::extract::extract_concepts(&store, &embedder, backend.as_ref()).await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        Cmd::Cluster => {
            let r = atlas::cluster::cluster(&store).await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        Cmd::Insights => {
            let r = atlas::analyze::analyze(&store).await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        Cmd::Query { text, limit } => {
            let r = atlas::query::query(&store, &text, limit).await?;
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        Cmd::Serve { port } => {
            let state = atlas::web::AtlasState {
                db,
                embedder: Arc::new(embedder),
                jobs: Default::default(),
            };
            let app = atlas::web::router(state);
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
            tracing::info!("atlas REST on http://127.0.0.1:{port}");
            axum::serve(listener, app).await?;
        }
    }
    Ok(())
}
