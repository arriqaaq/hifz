use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};

use hifz::Hifz;

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
            ollama_url: _,
            ollama_model: _,
        } => {
            // Note: --ollama-url and --ollama-model CLI args are kept for back-compat
            // but config is now loaded from ~/.hifz/.env via Hifz::open_*. To override,
            // set OLLAMA_URL / OLLAMA_MODEL env vars (recognised by `config::load_config`).
            tracing::info!("hifz v{} starting...", env!("CARGO_PKG_VERSION"));

            let hifz = if memory {
                tracing::info!("Storage: in-memory (ephemeral)");
                Hifz::open_memory().await?
            } else {
                tracing::info!("Storage: SurrealKV ({})", db_path);
                Hifz::open_persistent(&db_path).await?
            };

            tracing::info!("REST API: http://127.0.0.1:{port}/api/v1/*");
            tracing::info!("Embeddings: fastembed ({} dims)", hifz.embedder.dimension());
            if hifz.ollama.is_some() {
                tracing::info!("Ollama: enabled");
            } else {
                tracing::info!("Ollama: not configured (zero-LLM mode)");
            }
            if let Some(ref p) = hifz.git_path {
                tracing::info!("Git: {}", p.display());
            } else {
                tracing::warn!(
                    "git binary not found on PATH — commit enrichment \
                     (files_changed, insertions, deletions) will be unavailable"
                );
            }

            hifz::web::serve(hifz, port).await?;
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
