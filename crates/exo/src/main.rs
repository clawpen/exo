//! Containment - Container runtime for AI agents

mod commands;
mod metrics;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber;

#[derive(Parser, Debug)]
#[command(name = "exo")]
#[command(author = "Containment Contributors")]
#[command(version = "0.1.0")]
#[command(about = "Container runtime optimized for AI agents", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable debug logging
    #[arg(short, long, global = true)]
    debug: bool,

    /// Quiet mode (minimal output)
    #[arg(short, long, global = true)]
    quiet: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a container
    Run {
        /// Container image (e.g., python:3.12)
        image: String,

        /// Command to run (follows `--`)
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,

        /// Container name
        #[arg(short, long)]
        name: Option<String>,

        /// Config file (TOML)
        #[arg(short, long, value_name = "FILE")]
        config: Option<String>,

        /// Working directory
        #[arg(long, value_name = "DIR")]
        workdir: Option<String>,

        /// Volume mounts (source:target)
        #[arg(short, long, value_name = "SRC:DEST")]
        volume: Vec<String>,

        /// Environment variables (KEY=VALUE)
        #[arg(short, long, value_name = "KEY=VALUE")]
        env: Vec<String>,

        /// Enable GPU passthrough
        #[arg(long)]
        gpu: bool,

        /// GPU type (nvidia, amd, auto)
        #[arg(long, value_name = "TYPE")]
        gpu_type: Option<String>,

        /// Memory limit (e.g., 2G, 512M)
        #[arg(short, long, value_name = "LIMIT")]
        memory: Option<String>,

        /// CPU limit (e.g., 2, 200%)
        #[arg(long, value_name = "LIMIT")]
        cpu: Option<String>,

        /// Network mode (bridge, host, none)
        #[arg(long, value_name = "MODE")]
        network: Option<String>,

        /// Restart policy (no, on-failure, always)
        #[arg(long, value_name = "POLICY")]
        restart: Option<String>,

        /// Port mappings (host:container)
        #[arg(short, long, value_name = "HOST:CONT")]
        publish: Vec<String>,

        /// Healthcheck command (e.g., "curl -f http://localhost/")
        #[arg(long, value_name = "CMD")]
        health_cmd: Option<String>,

        /// Healthcheck interval in seconds
        #[arg(long, value_name = "SECS", default_value = "30")]
        health_interval: u64,

        /// Healthcheck timeout in seconds
        #[arg(long, value_name = "SECS", default_value = "30")]
        health_timeout: u64,

        /// Consecutive healthcheck failures before marking unhealthy
        #[arg(long, value_name = "N", default_value = "3")]
        health_retries: u32,

        /// Grace period in seconds before failed healthchecks count
        #[arg(long, value_name = "SECS", default_value = "0")]
        health_start_period: u64,

        /// Remove container on exit
        #[arg(long)]
        rm: bool,

        /// Interactive mode (keep STDIN open)
        #[arg(short, long)]
        interactive: bool,

        /// Allocate a pseudo-TTY
        #[arg(short, long)]
        tty: bool,

        /// Detach from container (run in background)
        #[arg(short, long)]
        detach: bool,
    },

    /// List running containers
    #[command(alias = "ps")]
    List {
        /// Show all containers (including stopped)
        #[arg(short, long)]
        all: bool,

        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Start a stopped container
    Start {
        /// Container ID or name
        container: String,

        /// Attach to container output
        #[arg(short, long)]
        attach: bool,
    },

    /// Stop a running container
    Stop {
        /// Container ID or name
        container: String,

        /// Force stop (SIGKILL)
        #[arg(short, long)]
        force: bool,

        /// Wait time before force killing (seconds)
        #[arg(short, long, default_value = "10")]
        time: u64,
    },

    /// Remove a container
    #[command(alias = "rm")]
    Remove {
        /// Container ID or name
        container: String,

        /// Force remove (even if running)
        #[arg(short, long)]
        force: bool,
    },

    /// View container logs
    Logs {
        /// Container ID or name
        container: String,

        /// Follow log output
        #[arg(short, long)]
        follow: bool,

        /// Show last N lines
        #[arg(short, long, default_value = "100")]
        tail: usize,

        /// Show timestamps
        #[arg(short, long)]
        timestamps: bool,
    },

    /// Execute a command in a running container
    Exec {
        /// Container ID or name
        container: String,

        /// Command to execute
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,

        /// Interactive mode
        #[arg(short, long)]
        interactive: bool,

        /// Allocate a pseudo-TTY
        #[arg(short, long)]
        tty: bool,

        /// User to run as
        #[arg(long, value_name = "USER")]
        user: Option<String>,
    },

    /// Pull an image
    Pull {
        /// Image to pull
        image: String,

        /// Skip vulnerability scan after pull
        #[arg(long)]
        skip_scan: bool,

        /// Verify image signature with cosign before pull
        #[arg(long)]
        verify: bool,

        /// Path to cosign public key for verification
        #[arg(long, value_name = "KEY")]
        cosign_key: Option<String>,

        /// Generate and save an SBOM after pull
        #[arg(long)]
        sbom: bool,
    },

    /// List images
    Images {
        /// Show all images (including intermediate)
        #[arg(short, long)]
        all: bool,
    },

    /// Build an image from an agent manifest (exo.toml) or a Dockerfile
    Build {
        /// Path to exo.toml/Dockerfile or a directory (default: ./exo.toml)
        #[arg(short, long)]
        file: Option<String>,

        /// Image name for Dockerfile builds (e.g. -t my-agent)
        #[arg(short, long)]
        tag: Option<String>,

        /// Skip vulnerability scan after build
        #[arg(long)]
        skip_scan: bool,

        /// Generate and save an SBOM after build
        #[arg(long)]
        sbom: bool,
    },

    /// Inspect a local image (layers, sizes, shared vs exclusive disk)
    Image {
        #[command(subcommand)]
        cmd: ImageCmd,
    },

    /// Push a locally-stored image to its registry
    Push {
        /// Image to push (e.g., ghcr.io/me/agent:latest)
        image: String,

        /// Sign the image with cosign after push
        #[arg(long)]
        sign: bool,

        /// Path to cosign private key for signing
        #[arg(long, value_name = "KEY")]
        cosign_key: Option<String>,
    },

    /// Remove an image (refcount-aware: prunes only its orphaned layers)
    Rmi {
        /// Image to remove (e.g., python:3.12)
        image: String,
    },

    /// Import image from tarball
    Import {
        /// Path to image tarball
        tarball: String,

        /// Name for imported image (e.g., myimage:latest)
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Show resource usage statistics for a container
    Stats {
        /// Container ID or name
        container: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Show detailed information about a container
    Inspect {
        /// Container ID or name
        container: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Copy files between host and a container rootfs
    Cp {
        /// Source path (host path or container:path)
        source: String,

        /// Destination path (host path or container:path)
        dest: String,
    },

    /// Show the daemon's lifecycle event log
    Events {
        /// Filter to one container (by id or name)
        #[arg(short, long)]
        container: Option<String>,

        /// Maximum events to show (newest first)
        #[arg(short, long, default_value = "50")]
        limit: usize,

        /// Output as JSON
        #[arg(long)]
        json: bool,

        /// Export events to a JSONL file instead of printing
        #[arg(long, value_name = "PATH")]
        export: Option<String>,
    },

    /// Show layer-store disk usage and dedup savings
    System {
        #[command(subcommand)]
        cmd: SystemCmd,
    },

    /// Daemon mode - run a persistent server for faster operations
    Daemon {
        /// Run in foreground (don't detach)
        #[arg(long)]
        foreground: bool,

        /// Stop the daemon
        #[arg(long)]
        stop: bool,

        /// Show daemon status
        #[arg(long)]
        status: bool,

        /// Socket path (default: /tmp/exo-daemon.sock)
        #[arg(long, value_name = "PATH")]
        socket: Option<String>,

        /// Request timeout in milliseconds
        #[arg(long, default_value = "30000")]
        timeout: u64,

        /// Show status in JSON format
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum SystemCmd {
    /// Show image-store disk usage and dedup savings
    Df {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove extracted layers no image references anymore
    Prune,
    /// Scan the image store for inconsistencies (optionally repair)
    Check {
        /// Remove dangling images and prune orphaned layers
        #[arg(long)]
        repair: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ImageCmd {
    /// Show an image's layers, sizes, and shared-vs-exclusive disk usage
    Inspect {
        /// Image to inspect (e.g., python:3.12)
        image: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = if cli.debug {
        "debug"
    } else if cli.quiet {
        "warn"
    } else {
        "info"
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    // Run the appropriate command
    match cli.command {
        Commands::Run { image, command, name, config, workdir, volume, env, gpu, gpu_type, memory, cpu, network, restart, publish, health_cmd, health_interval, health_timeout, health_retries, health_start_period, rm, interactive, tty, detach } => {
            commands::run::execute(commands::run::RunArgs {
                image,
                command,
                name,
                config,
                workdir,
                volume,
                env,
                gpu,
                gpu_type,
                memory,
                cpu,
                network,
                restart,
                publish,
                health_cmd,
                health_interval,
                health_timeout,
                health_retries,
                health_start_period,
                rm,
                interactive,
                tty,
                detach,
            }).await?
        }
        Commands::List { all, json } => {
            commands::list::execute(commands::list::ListArgs { all, json }).await?
        }
        Commands::Start { container, attach } => {
            commands::start::execute(commands::start::StartArgs { container, attach }).await?
        }
        Commands::Stop { container, force, time } => {
            commands::stop::execute(commands::stop::StopArgs { container, force, time }).await?
        }
        Commands::Remove { container, force } => {
            commands::remove::execute(commands::remove::RemoveArgs { container, force }).await?
        }
        Commands::Logs { container, follow, tail, timestamps } => {
            commands::logs::execute(commands::logs::LogsArgs { container, follow, tail, timestamps }).await?
        }
        Commands::Exec { container, command, interactive, tty, user } => {
            commands::exec::execute(commands::exec::ExecArgs { container, command, interactive, tty, user }).await?
        }
        Commands::Pull { image, skip_scan, verify, cosign_key, sbom } => {
            commands::pull::execute(commands::pull::PullArgs { image, skip_scan, verify, cosign_key, sbom }).await?
        }
        Commands::Images { all } => {
            commands::images::execute(commands::images::ImagesArgs { all }).await?
        }
        Commands::Build { file, tag, skip_scan, sbom } => {
            commands::build::execute(commands::build::BuildArgs { file, tag, skip_scan, sbom }).await?
        }
        Commands::Image { cmd } => match cmd {
            ImageCmd::Inspect { image, json } => {
                commands::image::inspect(commands::image::InspectArgs { image, json }).await?
            }
        }
        Commands::Push { image, sign, cosign_key } => {
            commands::push::execute(commands::push::PushArgs { image, sign, cosign_key }).await?
        }
        Commands::Rmi { image } => {
            commands::rmi::execute(commands::rmi::RmiArgs { image }).await?
        }
        Commands::Import { tarball, name } => {
            commands::import::execute(commands::import::ImportArgs {
                tarball: PathBuf::from(tarball),
                name,
            }).await?
        }
        Commands::Stats { container, json } => {
            commands::stats::execute(commands::stats::StatsArgs { container, json }).await?
        }
        Commands::Inspect { container, json } => {
            commands::inspect::execute(commands::inspect::InspectArgs { container, json }).await?
        }
        Commands::Cp { source, dest } => {
            commands::cp::execute(commands::cp::CpArgs { source, dest }).await?
        }
        Commands::Events { container, limit, json, export } => {
            commands::events::execute(commands::events::EventsArgs { container, limit, json, export }).await?
        }
        Commands::System { cmd } => match cmd {
            SystemCmd::Df { json } => {
                commands::system::df(commands::system::DfArgs { json }).await?
            }
            SystemCmd::Prune => commands::system::prune().await?,
            SystemCmd::Check { repair, json } => {
                commands::system::check(commands::system::CheckArgs { repair, json }).await?
            }
        }
        Commands::Daemon { foreground, stop, status, socket, timeout, json } => {
            if stop {
                commands::daemon::stop()?;
            } else if status {
                commands::daemon::status(commands::daemon::DaemonStatusArgs { json })?;
            } else {
                commands::daemon::start(commands::daemon::DaemonArgs {
                    socket_path: socket,
                    timeout: Some(timeout),
                    foreground,
                    stop: false,
                })?;
            }
        }
    }

    Ok(())
}
