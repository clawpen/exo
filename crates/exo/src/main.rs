//! Containment - Container runtime for AI agents

mod agent_docs;
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

    /// Output machine-readable JSON (agent contract, schema 1). On failure,
    /// a structured error envelope is printed to stderr and the process
    /// exits with the documented code (docs/EXIT_CODES.md).
    #[arg(long, global = true)]
    json: bool,
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

        /// Host workspace directory to stream into the container and pull back
        /// after the run (macOS Linux microVM backend only)
        #[arg(long, value_name = "DIR")]
        workspace: Option<String>,

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

        /// Runtime backend: auto, native, or linux
        #[arg(long, value_name = "BACKEND", default_value = "auto")]
        backend: String,
    },

    /// Start a stopped container
    Start {
        /// Container ID or name
        container: String,

        /// Attach to container output
        #[arg(short, long)]
        attach: bool,

        /// Runtime backend: auto, native, or linux
        #[arg(long, value_name = "BACKEND", default_value = "auto")]
        backend: String,
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

        /// Runtime backend: auto, native, or linux
        #[arg(long, value_name = "BACKEND", default_value = "auto")]
        backend: String,
    },

    /// Remove a container
    #[command(alias = "rm")]
    Remove {
        /// Container ID or name
        container: String,

        /// Force remove (even if running)
        #[arg(short, long)]
        force: bool,

        /// Runtime backend: auto, native, or linux
        #[arg(long, value_name = "BACKEND", default_value = "auto")]
        backend: String,
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

        /// Runtime backend: auto, native, or linux
        #[arg(long, value_name = "BACKEND", default_value = "auto")]
        backend: String,
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

        /// Runtime backend: auto, native, or linux
        #[arg(long, value_name = "BACKEND", default_value = "auto")]
        backend: String,
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
    Doctor,

    /// Show the daemon's lifecycle event log
    Events {
        /// Filter to one container (by id or name)
        #[arg(short, long)]
        container: Option<String>,

        /// Maximum events to show (newest first)
        #[arg(short, long, default_value = "50")]
        limit: usize,
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

    /// Print the generated agent CLI reference (source of docs/AGENT_CLI.md)
    #[command(hide = true)]
    AgentDocs,
}

#[derive(Subcommand, Debug)]
enum BackendCommands {
    /// Show active backend and capabilities
    Info,
}

#[derive(Subcommand, Debug)]
enum GpuCommands {
    /// List detected GPUs
    List,
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
    Status,

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

    /// Remove an image rootfs from the guest store
    RmImage {
        /// Image name/tag to remove from guest state
        image: String,
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
    List,

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
    List,

    /// Inspect a named volume
    Inspect {
        /// Volume name
        name: String,
    },

    /// Remove a named volume
    #[command(alias = "rm")]
    Remove {
        /// Volume name
        name: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    // Initialize tracing. In --json mode stderr carries the error envelope,
    // so log noise is suppressed to errors unless --debug is also given.
    let filter = if cli.debug {
        "debug"
    } else if cli.quiet || cli.json {
        "error"
    } else {
        "info"
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    // Exit-code + error-envelope contract (docs/EXIT_CODES.md): typed
    // ExoErrors carry their documented code even through anyhow chains;
    // anything untyped is an internal error (6). Never exit 1 on failure.
    // With --json, the structured envelope goes to stderr so stdout stays
    // pure data.
    let json = cli.json;
    match dispatch(cli.command, json).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            let code = exo_runtime::exit_code_for(&err);
            if json {
                eprintln!(
                    "{}",
                    serde_json::to_string(&exo_runtime::envelope_for(&err))
                        .unwrap_or_else(|_| "{\"schema\":1,\"error\":{\"code\":\"INTERNAL\",\"message\":\"serialization failure\",\"retryable\":false}}".to_string())
                );
            } else {
                eprintln!("Error: {err:#}");
            }
            std::process::ExitCode::from(code as u8)
        }
    }
}

async fn dispatch(command: Commands, json: bool) -> anyhow::Result<()> {
    // Run the appropriate command
    match command {
        Commands::Run {
            image,
            command,
            name,
            config,
            workdir,
            workspace,
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
                workspace,
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
                json,
            })
            .await?
        }
        Commands::List { all, backend } => {
            commands::list::execute(commands::list::ListArgs { all, json, backend }).await?
        }
        Commands::Start {
            container,
            attach,
            backend,
        } => {
            commands::start::execute(commands::start::StartArgs {
                container,
                attach,
                backend,
                json,
            })
            .await?
        }
        Commands::Stop {
            container,
            force,
            time,
            backend,
        } => {
            commands::stop::execute(commands::stop::StopArgs {
                container,
                force,
                time,
                backend,
                json,
            })
            .await?
        }
        Commands::Remove {
            container,
            force,
            backend,
        } => {
            commands::remove::execute(commands::remove::RemoveArgs {
                container,
                force,
                backend,
                json,
            })
            .await?
        }
        Commands::Logs {
            container,
            follow,
            tail,
            timestamps,
            backend,
        } => {
            commands::logs::execute(commands::logs::LogsArgs {
                container,
                follow,
                tail,
                timestamps,
                backend,
                json,
            })
            .await?
        }
        Commands::Exec {
            container,
            command,
            interactive,
            tty,
            user,
            backend,
        } => {
            commands::exec::execute(commands::exec::ExecArgs {
                container,
                command,
                interactive,
                tty,
                user,
                backend,
                json,
            })
            .await?
        }
        Commands::Pull { image } => {
            commands::pull::execute(commands::pull::PullArgs { image, json }).await?
        }
        Commands::Images { all } => {
            commands::images::execute(commands::images::ImagesArgs { all, json }).await?
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
            SecretCommands::List => {
                commands::secret::list(commands::secret::SecretListArgs { json }).await?
            }
            SecretCommands::Remove { name } => {
                commands::secret::remove(commands::secret::SecretRemoveArgs { name }).await?
            }
        },
        Commands::Doctor => {
            commands::doctor::execute(commands::doctor::DoctorArgs { json }).await?
        }
        Commands::Events {
            container,
            limit,
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
            BackendCommands::Info => {
                commands::backend::info(commands::backend::BackendInfoArgs { json }).await?
            }
        },
        Commands::Gpu { command } => match command {
            GpuCommands::List => {
                commands::gpu::list(commands::gpu::GpuListArgs { json }).await?
            }
        },
        Commands::Vm { command } => match command {
            VmCommands::Init { force } => commands::vm::init(force).await?,
            VmCommands::Start { foreground } => commands::vm::start(foreground).await?,
            VmCommands::Serve => commands::vm::serve().await?,
            VmCommands::Stop { force } => commands::vm::stop(force).await?,
            VmCommands::Status => commands::vm::status(json).await?,
            VmCommands::Reset { keep_state } => commands::vm::reset(keep_state).await?,
            VmCommands::InstallGuestAgent { path } => {
                commands::vm::install_guest_agent(path).await?
            }
            VmCommands::ImportImage { image, guest_path } => {
                commands::vm::import_image(image, guest_path).await?
            }
            VmCommands::RmImage { image } => commands::vm::remove_image(image).await?,
        },
        Commands::Volume { command } => match command {
            VolumeCommands::Create { name } => {
                commands::volume::create(commands::volume::VolumeCreateArgs { name }).await?
            }
            VolumeCommands::List => {
                commands::volume::list(commands::volume::VolumeListArgs { json }).await?
            }
            VolumeCommands::Inspect { name } => {
                commands::volume::inspect(commands::volume::VolumeInspectArgs { name, json })
                    .await?
            }
            VolumeCommands::Remove { name } => {
                commands::volume::remove(commands::volume::VolumeRemoveArgs { name }).await?
            }
        },
        Commands::AgentDocs => {
            // Meta command: markdown on stdout, always (it's a document, not
            // a data payload — no schema wrapper even in --json mode).
            print!("{}", agent_docs::render());
        }
    }

    Ok(())
}
