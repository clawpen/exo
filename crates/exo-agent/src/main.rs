//! Exo Agent - Lightweight agent runtime
//!
//! A minimal, fast agent runtime designed to run inside exo containers.
//! Provides LLM integration, memory, tools, and stdio communication.

use anyhow::Result;
use clap::Parser;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod agent;
mod channel;
mod config;
mod llm;
mod memory;
mod tools;

use agent::ExoAgent;
use config::AgentConfig;

#[derive(Parser)]
#[command(name = "exo-agent", author, version, about = "Exo Agent")]
struct Args {
    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    /// Agent name
    #[arg(short, long)]
    name: Option<String>,

    /// LLM model
    #[arg(short, long)]
    model: Option<String>,

    /// System prompt
    #[arg(short = 'S', long)]
    system: Option<String>,

    /// Enable debug logging
    #[arg(short, long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    info!("Exo Agent v{} starting", env!("CARGO_PKG_VERSION"));

    // Parse arguments
    let args = Args::parse();

    // Load configuration
    let mut config = if let Some(path) = &args.config {
        AgentConfig::from_file(std::path::Path::new(path))?
    } else {
        AgentConfig::from_env()?
    };

    // Override with CLI args
    if let Some(name) = args.name {
        config.name = name;
    }
    if let Some(model) = args.model {
        config.llm.model = model;
    }
    if let Some(system) = args.system {
        config.system_prompt = Some(system);
    }

    // Create and run agent
    let mut agent = ExoAgent::new(config).await?;

    // Handle shutdown gracefully
    tokio::select! {
        result = agent.run() => {
            result?;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
            agent.stop();
        }
    }

    Ok(())
}
