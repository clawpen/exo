//! Run command implementation

#[cfg(windows)]
use exo_wsl::{WslCommand, WslMount, WslDistroManager, NetworkManager, NetworkConfig, NetworkMode, PortMapping, PortProtocol, AgentNetworkConfig, WslGpuDetector, WslConfig, WslDeployer};
use exo_runtime::config::ContainerConfig;
use exo_runtime::image::{ImageManager, TagOrDigest};
use exo_runtime::storage::OverlayfsDriver;
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

    // If config file is specified, load it
    let config = if let Some(config_path) = args.config {
        load_config_from_file(&config_path)?
    } else {
        build_config_from_args(args)?
    };

    #[cfg(windows)]
    {
        // Windows path: Use WSL2 backend
        execute_windows(config, detach, rm).await
    }

    #[cfg(not(windows))]
    {
        // Linux path: Use native runtime with new features
        execute_linux(config, detach, rm).await
    }
}

#[cfg(windows)]
async fn execute_windows(config: ContainerConfig, detach: bool, rm: bool) -> anyhow::Result<()> {
    use exo_wsl::command::{ContainerSpec, MountSpec};
    use exo_wsl::networking::{NetworkManager, NetworkConfig, NetworkMode, PortMapping, PortProtocol, AgentNetworkConfig, DnsEntry};
    use exo_wsl::deploy::WslDeployer;
    use tracing::{info, debug};

    info!("Running container via WSL2 backend");

    // Initialize WSL environment
    let wsl_manager = WslDistroManager::new(WslConfig::default());
    wsl_manager.initialize()?;

    // Deploy runtime to WSL if needed
    let deployer = WslDeployer::new(WslConfig::default());
    deployer.ensure_wsl_ready()?;
    deployer.deploy_runtime()?;

    // Set up networking for agents
    let net_config = AgentNetworkConfig::default();
    let network_manager = NetworkManager::new(
        &net_config.bridge_name,
        &net_config.subnet,
        &net_config.gateway_ip,
    )?;

    // Create bridge network for agent communication
    network_manager.create_bridge()?;

    // Convert Windows paths to WSL paths for mounts
    let wsl_mount = WslMount::new(WslConfig::default());

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

    // Start container
    let container_id = wsl_cmd.start_container(&container_spec)?;

    info!("Container started: {}", container_id);
    info!("Network: {} @ {}", container_network.ip, config.name);

    if detach {
        println!("Container running in background: {}", container_id);
        println!("Agent reachable at {}:{}", container_network.ip, config.name);
        return Ok(());
    }

    // Wait for container to finish
    let status = wsl_cmd.container_status(&container_id)?;

    match status {
        exo_wsl::command::ContainerStatus::Running => {
            // Attach to logs
            let mut log_stream = wsl_cmd.stream_logs(&container_id).await?;
            while let Some(line) = log_stream.next_line().await? {
                println!("{}", line);
            }
            log_stream.wait().await?;
        }
        _ => {
            println!("Container status: {:?}", status);
        }
    }

    if rm {
        wsl_cmd.stop_container(&container_id)?;
    }

    Ok(())
}

#[cfg(not(windows))]
async fn execute_linux(config: ContainerConfig, detach: bool, rm: bool) -> anyhow::Result<()> {
    use exo_runtime::{Container, ContainerStatus};
    use exo_gpu::{GpuConfig, GpuType};

    // Initialize image manager and storage
    let image_manager = ImageManager::new()?;

    // Parse the image reference to check for foreign architecture
    let image_ref = image_manager.parse_image_reference(&config.image)?;
    let is_foreign = match &image_ref.reference {
        TagOrDigest::Tag(_) => {
            // Check if the tag indicates architecture
            config.image.contains("arm64") || config.image.contains("armv7")
        }
        _ => false
    };

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

    // Pull image if not present (with registry feature)
    #[cfg(feature = "registry")]
    {
        use exo_runtime::TagOrDigest;
        if let TagOrDigest::Tag(tag) = &image_ref.reference {
            let image_name = format!("{}/{}:{}", image_ref.registry, image_ref.repository, tag);
            if let Err(e) = image_manager.pull(&image_name).await {
                tracing::warn!("Failed to pull image: {}", e);
                // Continue anyway - image might be cached
            }
        }
    }

    // Create and start the container
    let mut container = Container::new(config)?;

    println!("Starting container: {}", container.handle().id);

    // Print architecture info if foreign
    if is_foreign {
        println!("Running with QEMU emulation for foreign architecture");
    }

    container.start()?;

    if detach {
        let pid = container.handle().pid.unwrap();
        println!("Container running in background: {}", container.handle().id);
        println!("PID: {}", pid);

        // Store container state for management
        // In production, this would write to a state file

        return Ok(());
    }

    // Wait for container to finish
    let status = container.wait()?;

    // Clean up overlay if needed
    let _ = image_manager.storage().remove_container_overlay(&container.handle().id);

    if rm {
        container.remove()?;
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
        format!("exo-{}", uuid::Uuid::new_v4().to_string()[..8].to_string())
    });

    let command = if args.command.is_empty() {
        vec!["sh".to_string()]
    } else {
        args.command.clone()
    };

    let workdir = args.workdir.unwrap_or_else(|| "/app".to_string());

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
