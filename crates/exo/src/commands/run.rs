//! Run command implementation

#[cfg(windows)]
use exo_wsl::{WslCommand, WslMount, WslDistroManager, NetworkManager, NetworkConfig, NetworkMode, PortMapping, PortProtocol, AgentNetworkConfig, WslGpuDetector, WslConfig, WslDeployer};
use exo_runtime::config::ContainerConfig;
use exo_runtime::{ContainerManager, ContainerMetadata};
use exo_image::ImageManager;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct RunArgs {
    pub image: String,
    pub command: Vec<String>,
    pub name: Option<String>,
    pub config: Option<String>,
    pub workdir: Option<String>,
    pub volume: Vec<String>,
    pub env: Vec<String>,
    pub gpu: bool,
    pub gpu_type: Option<String>,
    pub memory: Option<String>,
    pub cpu: Option<String>,
    pub network: Option<String>,
    pub publish: Vec<String>,
    pub rm: bool,
    pub interactive: bool,
    pub tty: bool,
    pub detach: bool,
}

pub async fn execute(args: RunArgs) -> anyhow::Result<()> {
    // Extract flags we need later
    let detach = args.detach;
    let rm = args.rm;
    let name = args.name.clone();

    // If config file is specified, load it
    let config = if let Some(config_path) = args.config {
        load_config_from_file(&config_path)?
    } else {
        build_config_from_args(args)?
    };

    #[cfg(windows)]
    {
        // Windows path: Use WSL2 backend
        execute_windows(config, detach, rm, name).await
    }

    #[cfg(not(windows))]
    {
        // Linux path: Use native runtime with new features
        execute_linux(config, detach, rm, name).await
    }
}

#[cfg(windows)]
async fn execute_windows(config: ContainerConfig, detach: bool, rm: bool, name: Option<String>) -> anyhow::Result<()> {
    use exo_wsl::command::{ContainerSpec, MountSpec};
    use exo_wsl::networking::{NetworkManager, NetworkConfig, NetworkMode, PortMapping, PortProtocol, AgentNetworkConfig, DnsEntry};
    use exo_wsl::deploy::WslDeployer;
    use exo_wsl::daemon_client::DaemonClient;
    use tracing::{info, debug};

    info!("Running container via WSL2 backend");

    // Check if we should use daemon mode
    let use_daemon = std::env::var("EXO_NO_DAEMON").is_err();

    if use_daemon {
        // Try daemon mode first for performance
        let daemon_client = DaemonClient::new(&exo_wsl::WslConfig::default());

        // Initialize WSL environment and deploy runtime first
        let wsl_manager = exo_wsl::WslDistroManager::new(exo_wsl::WslConfig::default());
        wsl_manager.initialize()?;

        let deployer = exo_wsl::WslDeployer::new(exo_wsl::WslConfig::default());
        deployer.ensure_wsl_ready()?;
        deployer.deploy_runtime()?;

        // Check if daemon is available
        if daemon_client.is_running() || daemon_client.ensure_running().is_ok() {
            info!("Using exo daemon for faster execution");
            return execute_with_daemon(config, detach, rm, name, daemon_client).await;
        } else {
            debug!("Daemon not available, using direct mode");
        }
    }

    // Initialize WSL environment
    let wsl_manager = WslDistroManager::new(WslConfig::default());
    wsl_manager.initialize()?;

    // Deploy runtime to WSL if needed
    let deployer = WslDeployer::new(WslConfig::default());
    deployer.ensure_wsl_ready()?;
    deployer.deploy_runtime()?;

    // Set up networking for agents
    let net_config = AgentNetworkConfig::default();
    let wsl_config = WslConfig::default();
    let network_manager = NetworkManager::from_config(
        &wsl_config,
        &net_config.bridge_name,
        &net_config.subnet,
        &net_config.gateway_ip,
    )?;

    // Create bridge network for agent communication
    network_manager.create_bridge()?;

    // Convert Windows paths to WSL paths for mounts
    let wsl_mount = WslMount::new(wsl_config.clone());

    let mounts: Vec<MountSpec> = config.mounts
        .iter()
        .map(|m| {
            let source = if m.source.contains(':') {
                // Windows path
                wsl_mount.windows_to_wsl(&m.source)
            } else {
                m.source.clone()
            };

            MountSpec {
                source,
                target: m.target.clone(),
                readonly: m.readonly,
            }
        })
        .collect();

    // Set up port mappings
    let port_mappings: Vec<PortMapping> = config.network.port_mappings
        .iter()
        .map(|p| PortMapping {
            host_port: p.host_port,
            container_port: p.container_port,
            protocol: match p.protocol.as_str() {
                "udp" => PortProtocol::Udp,
                _ => PortProtocol::Tcp,
            },
            host_ip: p.host_ip.clone(),
        })
        .collect();

    // Set up container networking
    let container_network = network_manager.setup_container_network(
        &config.name,
        &NetworkConfig {
            mode: NetworkMode::Bridge,
            port_mappings,
            ..Default::default()
        },
    )?;

    // Add hostname to DNS
    let dns_entry = DnsEntry::new(&config.name, &container_network.ip);

    // Write DNS entry to /etc/hosts
    let state_dir = deployer.state_dir();
    dns_entry.write_to_container(&state_dir)?;

    // Check GPU support
    let gpu_enabled = config.gpu.as_ref()
        .map(|g| !g.devices.is_empty())
        .unwrap_or(false);

    if gpu_enabled {
        let detector = WslGpuDetector::new(WslConfig::default());
        let gpus = detector.detect_host_gpus()?;
        if gpus.is_empty() {
            tracing::warn!("GPU requested but no GPUs detected");
        } else {
            info!("Found {} GPU(s)", gpus.len());
            for gpu in &gpus {
                debug!("  - {} ({})", gpu.name, format!("{:?}", gpu.vendor));
            }
        }
    }

    // Create container spec
    let container_spec = ContainerSpec {
        id: uuid::Uuid::new_v4().to_string(),
        name: config.name.clone(),
        image: config.image.clone(),
        command: config.command.clone(),
        workdir: config.workdir.to_string_lossy().to_string(),
        env: config.env.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect(),
        mounts,
        gpu: gpu_enabled,
        memory_mb: config.resources.memory.as_ref()
            .and_then(|m| parse_memory_mb(m).ok()),
        cpu_shares: config.resources.cpu_shares,
    };

    // Ensure state directory exists and clean up stale containers
    let wsl_cmd = WslCommand::new(WslConfig::default());
    wsl_cmd.exec(&format!("mkdir -p {}", deployer.state_dir()))?;
    deployer.cleanup_stale_containers()?;

    if detach {
        // Start container in detached mode
        let container_id = wsl_cmd.start_container(&container_spec)?;

        info!("Container started: {}", container_id);
        info!("Network: {} @ {}", container_network.ip, config.name);

        // Set up Windows port forwarding for published ports
        if !config.network.port_mappings.is_empty() {
            use exo_wsl::{WindowsPortForwarder, PortForwardingRule, PortProtocol};

            let forwarder = WindowsPortForwarder::new(WslConfig::default());

            for port_mapping in &config.network.port_mappings {
                let rule = PortForwardingRule {
                    host_port: port_mapping.host_port,
                    container_port: port_mapping.container_port,
                    protocol: if port_mapping.protocol == "udp" {
                        PortProtocol::Udp
                    } else {
                        PortProtocol::Tcp
                    },
                    listen_address: port_mapping.host_ip.clone(),
                };

                match forwarder.add_port_forward(&config.name, rule) {
                    Ok(()) => {
                        println!("Port forwarding: {}:{} -> {}", port_mapping.host_port, port_mapping.container_port, config.name);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to set up port forwarding: {}", e);
                    }
                }
            }
        }

        println!("Container running in background: {}", container_id);
        println!("Agent reachable at {}:{}", container_network.ip, config.name);
        return Ok(());
    }

    // Run container synchronously
    info!("Running container synchronously...");
    let (exit_code, stdout) = wsl_cmd.run_container_sync(&container_spec)?;

    // Print container output
    if !stdout.is_empty() {
        print!("{}", stdout);
    }

    if exit_code != 0 {
        anyhow::bail!("Container exited with code {}", exit_code);
    }

    Ok(())
}

#[cfg(windows)]
/// Execute container using the daemon (faster mode).
async fn execute_with_daemon(
    config: ContainerConfig,
    detach: bool,
    rm: bool,
    name: Option<String>,
    daemon_client: exo_wsl::DaemonClient,
) -> anyhow::Result<()> {
    use tracing::info;
    use exo_wsl::mount::WslMount;
    use exo_wsl::WslConfig;

    // Convert Windows paths to WSL paths for mounts
    let wsl_mount = WslMount::new(WslConfig::default());

    let mounts: Vec<exo_wsl::DaemonMountSpec> = config.mounts
        .iter()
        .map(|m| {
            let source = if m.source.contains(':') {
                wsl_mount.windows_to_wsl(&m.source)
            } else {
                m.source.clone()
            };

            exo_wsl::DaemonMountSpec {
                source,
                target: m.target.clone(),
                readonly: m.readonly,
            }
        })
        .collect();

    let env: Vec<String> = config.env.iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();

    // Create container spec for daemon
    let spec = exo_wsl::DaemonContainerSpec {
        id: uuid::Uuid::new_v4().to_string(),
        name: config.name.clone(),
        image: config.image.clone(),
        command: config.command.clone(),
        workdir: config.workdir.to_string_lossy().to_string(),
        env,
        mounts,
        gpu: config.gpu.is_some(),
        memory_mb: config.resources.memory.as_ref()
            .and_then(|m| parse_memory_mb(m).ok()),
        cpu_shares: config.resources.cpu_shares,
    };

    // Start container via daemon
    let result = daemon_client.run_container(&spec)?;

    if !result.success {
        anyhow::bail!("Failed to start container: {}", result.message);
    }

    println!("{}", result.message);

    if detach {
        return Ok(());
    }

    // For non-detach mode, we'd need to stream logs via daemon
    // For now, just note that container is running
    info!("Container started in detached mode");

    if rm {
        // Clean up container
        let _ = daemon_client.stop_container(&spec.id);
    }

    Ok(())
}

#[cfg(not(windows))]
async fn execute_linux(config: ContainerConfig, detach: bool, rm: bool, name: Option<String>) -> anyhow::Result<()> {
    use exo_runtime::{Container, ContainerStatus};
    use exo_gpu::{GpuConfig, GpuType};

    // If --detach and the daemon is running, delegate to it. The daemon is a
    // long-lived root supervisor — the container outlives our CLI invocation
    // (which is what --detach is supposed to mean). Inline detach is broken:
    // the spawned container inherits our terminal session and gets SIGHUP'd
    // when this process exits.
    if detach && std::path::Path::new("/tmp/exo-daemon.sock").exists() {
        match daemon_run_detached(&config).await {
            Ok(id_or_name) => {
                println!("{}", id_or_name);
                return Ok(());
            }
            Err(e) => {
                tracing::warn!("daemon delegation failed: {}; falling back to inline", e);
                // Fall through to inline path below.
            }
        }
    }

    // Initialize container manager for persistence
    let manager = ContainerManager::new()?;
    
    // Check if container name is already in use
    if let Some(ref container_name) = name {
        if manager.exists(container_name) {
            anyhow::bail!("Container name '{}' is already in use", container_name);
        }
    }

    // Initialize image manager
    let image_manager = ImageManager::new()?;

    // Parse the image reference
    let image_ref = image_manager.parse_image_reference(&config.image)?;
    
    // Check for foreign architecture
    let is_foreign = config.image.contains("arm64") || config.image.contains("armv7");

    // Validate GPU config if specified
    if let Some(gpu_cfg) = &config.gpu {
        let gpu_type = gpu_cfg.gpu_type.parse::<GpuType>().unwrap_or(GpuType::Auto);
        let gpu_config = GpuConfig {
            gpu_type,
            devices: gpu_cfg.devices.clone(),
            all: gpu_cfg.devices.iter().any(|d| d == "all"),
            compute_mode: gpu_cfg.compute_mode.clone(),
            library_paths: vec![],
        };
        gpu_config.validate()?;
    }

    // Ensure rootfs is ready (this downloads/extracts if needed)
    let _rootfs = match image_manager.ensure_rootfs(&image_ref).await {
        Ok(path) => {
            tracing::info!("Image rootfs ready at: {:?}", path);
            // Create a symlink so the runtime can find it (Linux only)
            #[cfg(unix)]
            {
                let link_path = std::path::PathBuf::from("/tmp/exo-images/rootfs")
                    .join(config.image.replace(['/', ':'], "_"));
                if !link_path.exists() {
                    let _ = std::fs::create_dir_all(link_path.parent().unwrap());
                    let _ = std::os::unix::fs::symlink(&path, &link_path);
                }
            }
            path
        }
        Err(e) => {
            tracing::warn!("Could not prepare image rootfs: {}. Using fallback.", e);
            // Will use the bind mount fallback
            std::path::PathBuf::new()
        }
    };

    // Create container metadata for persistence
    let mut metadata = ContainerMetadata::new(config.name.clone(), config.clone());
    
    // Add port mappings to metadata for display
    metadata.ports = config.network.port_mappings
        .iter()
        .map(|p| format!("{}:{}", p.host_port, p.container_port))
        .collect();

    // Create and start the container
    let mut container = Container::new(config)?;

    println!("Starting container: {}", container.handle().id);

    // Print architecture info if foreign
    if is_foreign {
        println!("Running with QEMU emulation for foreign architecture");
    }

    container.start()?;

    // Update metadata with running state
    let pid = container.handle().pid.unwrap();
    metadata.id = container.handle().id.clone();
    metadata.set_running(pid);

    // Save container metadata (for persistence)
    if !rm {
        manager.save(&metadata)?;
    }

    if detach {
        println!("Container running in background: {}", container.handle().id);
        println!("PID: {}", pid);
        if let Some(ref container_name) = name {
            println!("Name: {}", container_name);
        }

        return Ok(());
    }

    // Wait for container to finish
    let status = container.wait()?;
    
    // Update metadata with exit status
    if let ContainerStatus::Exited(code) = status {
        metadata.set_stopped(Some(code));
    } else {
        metadata.set_stopped(None);
    }

    if rm {
        container.remove()?;
        // Also remove metadata
        if let Some(ref container_name) = name {
            let _ = manager.remove(container_name);
        }
    } else {
        // Save final state
        manager.save(&metadata)?;
    }

    match status {
        ContainerStatus::Exited(code) => {
            if code == 0 {
                Ok(())
            } else {
                Err(anyhow::anyhow!("Container exited with code {}", code))
            }
        }
        _ => Ok(()),
    }
}

fn parse_memory_mb(size: &str) -> anyhow::Result<u64> {
    let size = size.trim().to_lowercase();

    let (num, unit) = if size.ends_with('g') {
        (size.trim_end_matches('g'), "g")
    } else if size.ends_with('m') {
        (size.trim_end_matches('m'), "m")
    } else {
        (&size[..], "b")
    };

    let num: u64 = num.parse().map_err(|_| anyhow::anyhow!("Invalid size: {}", size))?;

    Ok(match unit {
        "g" => num * 1024,
        "m" => num,
        _ => num / 1024 / 1024,
    })
}

fn load_config_from_file(path: &str) -> anyhow::Result<ContainerConfig> {
    let content = std::fs::read_to_string(path)?;
    let config: toml::Value = toml::from_str(&content)?;

    let name = config.get("container")
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("exo-agent")
        .to_string();

    let image = config.get("container")
        .and_then(|c| c.get("image"))
        .and_then(|i| i.as_str())
        .unwrap_or("python:3.12-slim")
        .to_string();

    let workdir = config.get("container")
        .and_then(|c| c.get("runtime"))
        .and_then(|r| r.get("workdir"))
        .and_then(|w| w.as_str())
        .unwrap_or("/app")
        .to_string();

    let command = config.get("process")
        .and_then(|p| p.get("command"))
        .and_then(|c| c.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["python".to_string()]);

    let mut env = HashMap::new();
    if let Some(runtime) = config.get("container").and_then(|c| c.get("runtime")) {
        if let Some(env_arr) = runtime.get("env").and_then(|e| e.as_array()) {
            for env_var in env_arr {
                if let Some(s) = env_var.as_str() {
                    if let Some((key, value)) = s.split_once('=') {
                        env.insert(key.to_string(), value.to_string());
                    }
                }
            }
        }
    }

    Ok(ContainerConfig {
        name,
        image,
        workdir: PathBuf::from(workdir),
        env,
        command,
        ..Default::default()
    })
}

fn build_config_from_args(args: RunArgs) -> anyhow::Result<ContainerConfig> {
    use exo_runtime::config::{ResourceConfig, NetworkConfig, MountConfig, PortMapping, GpuConfig};

    let name = args.name.unwrap_or_else(|| {
        // Generate a name if not provided
        format!("exo-{}", uuid::Uuid::new_v4().to_string()[..8].to_string())
    });

    let command = if args.command.is_empty() {
        vec!["sh".to_string()]
    } else {
        args.command.clone()
    };

    let workdir = args.workdir.unwrap_or_else(|| "/".to_string());

    let mut env = HashMap::new();
    for e in &args.env {
        if let Some((key, value)) = e.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        }
    }

    let mut mounts = vec![];
    for v in &args.volume {
        if let Some((src, target)) = v.split_once(':') {
            mounts.push(MountConfig {
                mount_type: "bind".to_string(),
                source: src.to_string(),
                target: target.to_string(),
                readonly: false,
                size: None,
                propagation: "rprivate".to_string(),
            });
        }
    }

    let mut port_mappings = vec![];
    for p in &args.publish {
        if let Some((host, container)) = p.split_once(':') {
            let host_port = host.parse().unwrap_or(8080);
            let container_port = container.parse().unwrap_or(8080);
            port_mappings.push(PortMapping {
                host_port,
                container_port,
                protocol: "tcp".to_string(),
                host_ip: "0.0.0.0".to_string(),
            });
        }
    }

    let gpu = if args.gpu {
        Some(GpuConfig {
            gpu_type: args.gpu_type.clone().unwrap_or("auto".to_string()),
            devices: vec!["all".to_string()],
            compute_mode: None,
        })
    } else {
        None
    };

    let resources = ResourceConfig {
        memory: args.memory.clone(),
        cpu: args.cpu.clone(),
        ..Default::default()
    };

    let network = NetworkConfig {
        mode: args.network.clone().unwrap_or_else(|| "bridge".to_string()),
        port_mappings,
        ..Default::default()
    };

    Ok(ContainerConfig {
        name,
        image: args.image.clone(),
        workdir: PathBuf::from(workdir),
        env,
        user: "root".to_string(),
        command,
        resources,
        network,
        mounts,
        gpu,
        ..Default::default()
    })
}

/// Send a `Run` request to the daemon at `/tmp/exo-daemon.sock` and return
/// the container name (which serves as a stable identifier for downstream
/// stop/list/exec calls). Caller is responsible for checking that the
/// socket exists; this just connects and speaks the line-delimited JSON
/// protocol the daemon expects.
#[cfg(not(windows))]
async fn daemon_run_detached(config: &ContainerConfig) -> anyhow::Result<String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let env_strs: Vec<String> = config
        .env
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    let mounts_json: Vec<serde_json::Value> = config
        .mounts
        .iter()
        .map(|m| {
            serde_json::json!({
                "source": m.source,
                "target": m.target,
                "readonly": m.readonly,
            })
        })
        .collect();

    let req = serde_json::json!({
        "type": "run",
        "content": {
            "spec": {
                "name": config.name,
                "image": config.image,
                "workdir": config.workdir.to_string_lossy(),
                "env": env_strs,
                "command": config.command,
                "mounts": mounts_json,
            }
        }
    });

    let mut stream = UnixStream::connect("/tmp/exo-daemon.sock")
        .map_err(|e| anyhow::anyhow!("connect daemon socket: {}", e))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.write_all(req.to_string().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let resp: serde_json::Value = serde_json::from_str(line.trim())?;
    let resp_type = resp.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match resp_type {
        "ok" => Ok(config.name.clone()),
        "error" => {
            let msg = resp
                .get("content")
                .and_then(|c| c.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("daemon returned error");
            anyhow::bail!("{}", msg)
        }
        other => anyhow::bail!("unexpected daemon response type: {}", other),
    }
}
