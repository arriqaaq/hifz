use anyhow::Result;
use clap::Parser;

mod client;
mod config;
mod harness;
mod hash;
mod jsonl;
mod promote;
mod spool;
mod ui;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,pi_rpc_adapter=debug")),
        )
        .init();

    let cfg = config::Config::parse();
    harness::run(cfg).await
}
