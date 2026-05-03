use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use surrealdb::types::SurrealValue;

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
    /// Phase 4.6: dump every memory in a project to a directory of
    /// frontmatter-rich markdown files. The directory layout is flat:
    /// `<out>/<id>.md`. Open the directory in Obsidian or any editor;
    /// hand-edits round-trip via `hifz import`.
    Export {
        /// SurrealDB data directory
        #[arg(long, default_value = "db_data")]
        db_path: String,
        /// Project name to export. Use `--all` for every project.
        #[arg(long, conflicts_with = "all")]
        project: Option<String>,
        /// Export all memories regardless of project.
        #[arg(long, conflicts_with = "project")]
        all: bool,
        /// Output directory. Created if missing.
        #[arg(long)]
        out: PathBuf,
    },
    /// Phase 4.6: ingest a directory of edited markdown files. Each file
    /// must have an `id:` frontmatter field referring to a memory; the
    /// import writes a NEW version that supersedes the old.
    Import {
        /// SurrealDB data directory
        #[arg(long, default_value = "db_data")]
        db_path: String,
        /// Input directory. Every `*.md` file (non-recursive) is imported.
        #[arg(long)]
        from: PathBuf,
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

        Command::Export {
            db_path,
            project,
            all,
            out,
        } => {
            tracing::info!("hifz export — SurrealKV ({db_path}) → {}", out.display());
            std::fs::create_dir_all(&out)?;
            let hifz = Hifz::open_persistent(&db_path).await?;

            #[derive(Debug, surrealdb::types::SurrealValue)]
            struct IdRow {
                id: Option<surrealdb::types::RecordId>,
            }
            let sql = if all {
                "SELECT id FROM memory WHERE is_latest = true".to_string()
            } else {
                let p = project.clone().unwrap_or_else(|| "global".to_string());
                format!(
                    "SELECT id FROM memory WHERE is_latest = true AND (project = '{}' OR project = 'global')",
                    p.replace('\'', "")
                )
            };
            let mut resp = hifz.db.query(&sql).await?;
            let rows: Vec<IdRow> = resp.take(0).unwrap_or_default();

            let mut written = 0usize;
            for r in rows {
                let Some(rid) = r.id else { continue };
                let id_str = format!("{rid:?}");
                match hifz.memory_markdown_get(&id_str).await {
                    Ok(md) => {
                        let safe_name = id_str.replace([':', '/', '\\'], "_");
                        let path = out.join(format!("{safe_name}.md"));
                        std::fs::write(&path, md)?;
                        written += 1;
                    }
                    Err(e) => tracing::warn!("export skipped {id_str}: {e}"),
                }
            }
            println!("exported {written} memories to {}", out.display());
        }

        Command::Import { db_path, from } => {
            tracing::info!("hifz import — SurrealKV ({db_path}) ← {}", from.display());
            let hifz = Hifz::open_persistent(&db_path).await?;

            let mut imported = 0usize;
            let mut skipped = 0usize;
            for entry in std::fs::read_dir(&from)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                let body = std::fs::read_to_string(&path)?;
                let doc = match hifz::markdown::parse(&body) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("parse skipped {}: {e}", path.display());
                        skipped += 1;
                        continue;
                    }
                };
                let Some(id) = doc.frontmatter.id.clone() else {
                    tracing::warn!(
                        "skipped {}: no `id:` in frontmatter (use `hifz export` to seed)",
                        path.display()
                    );
                    skipped += 1;
                    continue;
                };
                match hifz.memory_markdown_put(&id, &body).await {
                    Ok(_) => imported += 1,
                    Err(e) => {
                        tracing::warn!("import failed {}: {e}", path.display());
                        skipped += 1;
                    }
                }
            }
            println!("imported {imported} memories ({skipped} skipped)");
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
