//! Containment - Container runtime for AI agents

mod commands;

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
    #[arg(long, global = true)]
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

        /// Secret names to inject from `exo secret` as environment variables
        #[arg(long, value_name = "NAME")]
        secret: Vec<String>,

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

        /// Port mappings (host:container)
        #[arg(short, long, value_name = "HOST:CONT")]
        publish: Vec<String>,

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

        /// Runtime backend: auto, native, or linux
        #[arg(long, value_name = "BACKEND", default_value = "auto")]
        backend: String,

        /// Host sandbox mode: auto, off, or required
        #[arg(long, value_name = "MODE", default_value = "auto")]
        sandbox: String,
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
        #[arg(long)]
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
    },

    /// List images
    Images {
        /// Show all images (including intermediate)
        #[arg(short, long)]
        all: bool,
    },

    /// Import image from tarball
    Import {
        /// Path to image tarball
        tarball: String,

        /// Name for imported image (e.g., myimage:latest)
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Manage local Exo secrets
    Secret {
        #[command(subcommand)]
        command: SecretCommands,
    },

    /// Diagnose host readiness for Exo
    Doctor {
        /// Output as JSON
        #[arg(long)]
        json: bool,
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

    /// Show backend information and capabilities
    Backend {
        #[command(subcommand)]
        command: BackendCommands,
    },

    /// Inspect GPU availability
    Gpu {
        #[command(subcommand)]
        command: GpuCommands,
    },

    /// Manage the Exo Linux microVM (macOS only)
    #[command(alias = "machine")]
    Vm {
        #[command(subcommand)]
        command: VmCommands,
    },

    /// Manage named volumes
    Volume {
        #[command(subcommand)]
        command: VolumeCommands,
    },
}

#[derive(Subcommand, Debug)]
enum BackendCommands {
    /// Show active backend and capabilities
    Info {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum GpuCommands {
    /// List detected GPUs
    List {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum VmCommands {
    /// Download/build the guest image
    Init {
        /// Force re-download and rebuild
        #[arg(long)]
        force: bool,
    },

    /// Start the VM
    Start {
        /// Run in foreground and attach to VM output
        #[arg(long)]
        foreground: bool,
    },

    /// Internal: run the VM control daemon in this process
    #[command(hide = true)]
    Serve,

    /// Stop the VM
    Stop {
        /// Force stop
        #[arg(short, long)]
        force: bool,
    },

    /// Show VM status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Reset the VM image and state
    Reset {
        /// Keep runtime state file
        #[arg(long)]
        keep_state: bool,
    },

    /// Install a built guest agent binary for embedding during `exo vm init`
    InstallGuestAgent {
        /// Path to exo-vm-guest-init built for the Linux guest architecture
        path: PathBuf,
    },

    /// Import an image rootfs tarball already visible inside the guest VM
    ImportImage {
        /// Image name/tag to register in guest state
        image: String,

        /// Path to a tar or tar.gz archive inside the guest VM
        #[arg(long, value_name = "PATH")]
        guest_path: String,
    },
}

#[derive(Subcommand, Debug)]
enum SecretCommands {
    /// Store a secret value (from --value, matching env var, or stdin)
    Set {
        /// Secret name
        name: String,

        /// Secret value; omit to read env var with the same name or stdin
        #[arg(long)]
        value: Option<String>,
    },

    /// List secret names (values are never printed)
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Remove a secret
    Remove {
        /// Secret name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum VolumeCommands {
    /// Create a named volume
    Create {
        /// Volume name
        name: String,
    },

    /// List named volumes
    #[command(alias = "ls")]
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Inspect a named volume
    Inspect {
        /// Volume name
        name: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Remove a named volume
    #[command(alias = "rm")]
    Remove {
        /// Volume name
        name: String,
    },
}

#[tokio::main(flavor = "current_thread")]
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
        Commands::Run {
            image,
            command,
            name,
            config,
            workdir,
            volume,
            env,
            secret,
            gpu,
            gpu_type,
            memory,
            cpu,
            network,
            publish,
            rm,
            interactive,
            tty,
            detach,
            backend,
            sandbox,
        } => {
            commands::run::execute(commands::run::RunArgs {
                image,
                command,
                name,
                config,
                workdir,
                volume,
                env,
                secret,
                gpu,
                gpu_type,
                memory,
                cpu,
                network,
                publish,
                rm,
                interactive,
                tty,
                detach,
                backend,
                sandbox,
            })
            .await?
        }
        Commands::List { all, json } => {
            commands::list::execute(commands::list::ListArgs { all, json }).await?
        }
        Commands::Start { container, attach } => {
            commands::start::execute(commands::start::StartArgs { container, attach }).await?
        }
        Commands::Stop {
            container,
            force,
            time,
        } => {
            commands::stop::execute(commands::stop::StopArgs {
                container,
                force,
                time,
            })
            .await?
        }
        Commands::Remove { container, force } => {
            commands::remove::execute(commands::remove::RemoveArgs { container, force }).await?
        }
        Commands::Logs {
            container,
            follow,
            tail,
            timestamps,
        } => {
            commands::logs::execute(commands::logs::LogsArgs {
                container,
                follow,
                tail,
                timestamps,
            })
            .await?
        }
        Commands::Exec {
            container,
            command,
            interactive,
            tty,
            user,
        } => {
            commands::exec::execute(commands::exec::ExecArgs {
                container,
                command,
                interactive,
                tty,
                user,
            })
            .await?
        }
        Commands::Pull { image } => {
            commands::pull::execute(commands::pull::PullArgs { image }).await?
        }
        Commands::Images { all } => {
            commands::images::execute(commands::images::ImagesArgs { all }).await?
        }
        Commands::Import { tarball, name } => {
            commands::import::execute(commands::import::ImportArgs {
                tarball: PathBuf::from(tarball),
                name,
            })
            .await?
        }
        Commands::Secret { command } => match command {
            SecretCommands::Set { name, value } => {
                commands::secret::set(commands::secret::SecretSetArgs { name, value }).await?
            }
            SecretCommands::List { json } => {
                commands::secret::list(commands::secret::SecretListArgs { json }).await?
            }
            SecretCommands::Remove { name } => {
                commands::secret::remove(commands::secret::SecretRemoveArgs { name }).await?
            }
        },
        Commands::Doctor { json } => {
            commands::doctor::execute(commands::doctor::DoctorArgs { json }).await?
        }
        Commands::Events {
            container,
            limit,
            json,
        } => {
            commands::events::execute(commands::events::EventsArgs {
                container,
                limit,
                json,
            })
            .await?
        }
        Commands::Daemon {
            foreground,
            stop,
            status,
            socket,
            timeout,
            json,
        } => {
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
        Commands::Backend { command } => match command {
            BackendCommands::Info { json } => {
                commands::backend::info(commands::backend::BackendInfoArgs { json }).await?
            }
        },
        Commands::Gpu { command } => match command {
            GpuCommands::List { json } => {
                commands::gpu::list(commands::gpu::GpuListArgs { json }).await?
            }
        },
        Commands::Vm { command } => match command {
            VmCommands::Init { force } => commands::vm::init(force).await?,
            VmCommands::Start { foreground } => commands::vm::start(foreground).await?,
            VmCommands::Serve => commands::vm::serve().await?,
            VmCommands::Stop { force } => commands::vm::stop(force).await?,
            VmCommands::Status { json } => commands::vm::status(json).await?,
            VmCommands::Reset { keep_state } => commands::vm::reset(keep_state).await?,
            VmCommands::InstallGuestAgent { path } => {
                commands::vm::install_guest_agent(path).await?
            }
            VmCommands::ImportImage { image, guest_path } => {
                commands::vm::import_image(image, guest_path).await?
            }
        },
        Commands::Volume { command } => match command {
            VolumeCommands::Create { name } => {
                commands::volume::create(commands::volume::VolumeCreateArgs { name }).await?
            }
            VolumeCommands::List { json } => {
                commands::volume::list(commands::volume::VolumeListArgs { json }).await?
            }
            VolumeCommands::Inspect { name, json } => {
                commands::volume::inspect(commands::volume::VolumeInspectArgs { name, json })
                    .await?
            }
            VolumeCommands::Remove { name } => {
                commands::volume::remove(commands::volume::VolumeRemoveArgs { name }).await?
            }
        },
    }

    Ok(())
}
