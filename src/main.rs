use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use clap::Parser;
use mcpl::{config::Config, registry::Registry, server::ProxyServer};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// JSON file containing an mcpServers object.
    #[arg(long, env = "MCPL_CONFIG")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "mcpl=info".into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    let mut registry = Arc::new(Registry::connect(config).await);
    let result = async {
        let service = ProxyServer::new(registry.clone())
            .serve(stdio())
            .await
            .context("upstream MCP handshake failed")?;
        let cancellation = service.cancellation_token();
        let waiting = service.waiting();
        tokio::pin!(waiting);
        tokio::select! {
            result = &mut waiting => { result.context("upstream MCP service failed")?; }
            result = tokio::signal::ctrl_c() => {
                cancellation.cancel();
                waiting.await.context("upstream MCP shutdown failed")?;
                result.context("could not listen for Ctrl-C")?;
            }
        }
        anyhow::Ok(())
    }
    .await;
    if let Some(registry) = Arc::get_mut(&mut registry) {
        registry.shutdown().await;
    }
    result
}
