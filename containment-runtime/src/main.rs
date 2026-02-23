//! OpenClaw Linux Runtime
//!
//! This binary runs inside WSL2 and manages containers using Linux kernel features.
//!
//! Usage:
//!   openclaw-runtime run --config <json> [--id-only]
//!   openclaw-runtime stop <container-id>
//!   openclaw-runtime status <container-id>
//!   openclaw-runtime logs [-f] <container-id>
//!   openclaw-runtime list

mod container;
mod namespaces;
mod cgroup;
mod mount;
mod rootfs;
mod process;
mod state;
mod server;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, error};
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(name = "openclaw-runtime")]
#[command(version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a new container
    Run {
        /// Container configuration as JSON
        #[arg(short, long)]
        config: String,

        /// Only output the container ID (for scripting)
        #[arg(long)]
        id_only: bool,
    },
    /// Stop a running container
    Stop {
        /// Container ID
        container_id: String,
    },
    /// Get container status
    Status {
        /// Container ID
        container_id: String,
    },
    /// Follow container logs
    Logs {
        /// Container ID
        container_id: String,

        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
    /// List all containers
    List,
    /// Start the RPC server (for communication with Windows)
    Server {
        /// Socket path
        #[arg(short, long, default_value = "/var/run/openclaw.sock")]
        socket: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("OPENCLAW_LOG")
                .unwrap_or_else(|_| "info".to_string())
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { config, id_only } => {
            cmd_run(config, id_only).await
        }
        Commands::Stop { container_id } => {
            cmd_stop(container_id).await
        }
        Commands::Status { container_id } => {
            cmd_status(container_id).await
        }
        Commands::Logs { container_id, follow } => {
            cmd_logs(container_id, follow).await
        }
        Commands::List => {
            cmd_list().await
        }
        Commands::Server { socket } => {
            server::run(&socket).await
        }
    }
}

async fn cmd_run(config: String, id_only: bool) -> Result<()> {
    use container::Container;

    let spec: serde_json::Value = serde_json::from_str(&config)?;
    let mut container = Container::from_spec(spec)?;

    let id = container.id().to_string();

    if id_only {
        println!("{}", id);
        return Ok(());
    }

    tracing::info!("Starting container: {}", id);

    container.start()?;

    // Write container state
    state::save_state(&id, "running")?;

    println!("{}", id);
    Ok(())
}

async fn cmd_stop(container_id: String) -> Result<()> {
    tracing::info!("Stopping container: {}", container_id);

    // TODO: Load actual container and stop it
    state::update_status(&container_id, "stopped")?;
    println!("Container stopped");

    Ok(())
}

async fn cmd_status(container_id: String) -> Result<()> {
    if let Some(status) = state::get_status(&container_id)? {
        println!("{}", status);
    } else {
        println!("unknown");
    }
    Ok(())
}

async fn cmd_logs(container_id: String, follow: bool) -> Result<()> {
    // For now, just read the log file
    let log_path = format!("/var/lib/openclaw/containers/{}/logs/container.log", container_id);

    if follow {
        // TODO: implement tail -f
        eprintln!("Follow not yet implemented");
    }

    if let Ok(content) = std::fs::read_to_string(&log_path) {
        print!("{}", content);
    }

    Ok(())
}

async fn cmd_list() -> Result<()> {
    let containers = state::list_containers()?;

    println!("CONTAINER ID\tNAME\tIMAGE\t\tSTATUS");
    for (id, info) in containers {
        println!("{}\t{}\t{}\t{}", id, info.name, info.image, info.status);
    }

    Ok(())
}
