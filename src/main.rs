use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "hifz",
    version,
    about = "Persistent memory for AI coding agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the full server (REST API + viewer)
    Serve {
        /// REST API port
        #[arg(long, default_value = "3111", env = "HIFZ_PORT")]
        port: u16,
        /// SurrealDB data directory (ignored with --memory)
        #[arg(long, default_value = "db_data")]
        db_path: String,
        /// Use in-memory storage (ephemeral, data lost on restart)
        #[arg(long)]
        memory: bool,
        /// Optional Ollama URL for LLM features
        #[arg(long, env = "OLLAMA_URL")]
        ollama_url: Option<String>,
        /// Ollama model name
        #[arg(long, env = "OLLAMA_MODEL", default_value = "qwen2.5:7b")]
        ollama_model: String,
    },
    /// Run MCP server over stdio (proxies to REST server)
    Mcp {
        /// REST server URL to proxy to
        #[arg(long, env = "HIFZ_URL", default_value = "http://localhost:3111")]
        url: String,
    },
    /// Show health status
    Status,
    /// Backfill schema upgrades (embeddings, project, ...) for existing memories
    Reindex {
        /// SurrealDB data directory
        #[arg(long, default_value = "db_data")]
        db_path: String,
        /// Reindex memories (hifz table): embeddings + project backfill
        #[arg(long)]
        memories: bool,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hifz=info".into()),
        )
        .init();

    let cli = Cli::parse();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(64 * 1024 * 1024)
        .build()?;

    runtime.block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> Result<()> {
    match cli.command.unwrap_or(Command::Serve {
        port: 3111,
        db_path: "db_data".to_string(),
        memory: false,
        ollama_url: None,
        ollama_model: "qwen2.5:7b".to_string(),
    }) {
        Command::Serve {
            port,
            db_path,
            memory,
            ollama_url,
            ollama_model,
        } => {
            tracing::info!("hifz v{} starting...", env!("CARGO_PKG_VERSION"));

            let db = if memory {
                tracing::info!("Storage: in-memory (ephemeral)");
                hifz::db::connect_mem().await?
            } else {
                tracing::info!("Storage: SurrealKV ({})", db_path);
                hifz::db::connect(&db_path).await?
            };

            let embedder = Arc::new(hifz::embed::Embedder::new()?);
            hifz::db::init_schema(&db, embedder.dimension()).await?;

            let ollama = if let Some(ref url) = ollama_url {
                tracing::info!("Ollama: {url} (model: {ollama_model})");
                Some(Arc::new(hifz::ollama::OllamaClient::new(
                    Some(url.clone()),
                    Some(ollama_model),
                )))
            } else {
                tracing::info!("Ollama: not configured (zero-LLM mode)");
                None
            };

            let git_path = which::which("git").ok();
            if let Some(ref p) = git_path {
                tracing::info!("Git: {}", p.display());
            } else {
                tracing::warn!(
                    "git binary not found on PATH — commit enrichment \
                     (files_changed, insertions, deletions) will be unavailable"
                );
            }

            let config = hifz::config::load_config();
            tracing::info!("REST API: http://127.0.0.1:{port}/api/v1/*");
            tracing::info!("Embeddings: fastembed ({} dims)", embedder.dimension());

            hifz::web::serve(
                db,
                port,
                embedder,
                ollama,
                config.auto_compress,
                config.token_budget,
                config.llm_evolve,
                git_path,
            )
            .await?;
        }

        Command::Mcp { url } => {
            let state = hifz::mcp::McpState {
                client: reqwest::Client::new(),
                base_url: url.clone(),
            };

            eprintln!("[hifz] MCP proxy → {url}");
            hifz::mcp::serve_stdio(state).await?;
        }

        Command::Reindex {
            db_path,
            memories: _,
        } => {
            tracing::info!("hifz reindex — SurrealKV ({db_path})");
            let db = hifz::db::connect(&db_path).await?;
            let embedder = Arc::new(hifz::embed::Embedder::new()?);
            hifz::db::init_schema(&db, embedder.dimension()).await?;

            let report = hifz::reindex::reindex_memories(&db, &embedder).await?;
            println!(
                "memories: embedded={}, project_backfilled={}, skipped={}",
                report.embedded, report.project_backfilled, report.skipped
            );
        }

        Command::Status => {
            let client = reqwest::Client::new();
            match client
                .get("http://127.0.0.1:3111/api/v1/health")
                .send()
                .await
            {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await?;
                    println!("{}", serde_json::to_string_pretty(&body)?);
                }
                Err(e) => {
                    eprintln!("Server not running: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}
