//! Main container implementation.

use crate::config::ContainerConfig;
use crate::process::ContainerProcess;
use crate::cgroup::CgroupManager;
use crate::rootfs;
use crate::healthcheck::HealthcheckRunner;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use crate::process::enter_container_namespaces;
use anyhow::Result;
use std::path::PathBuf;
use uuid::Uuid;

#[cfg(target_os = "linux")]
use nix::unistd::Pid;

/// Container handle - represents a running or stopped container.
#[derive(Debug, Clone)]
pub struct ContainerHandle {
    /// Unique container ID
    pub id: String,

    /// Container name
    pub name: String,

    /// Process ID
    pub pid: Option<u32>,

    /// Container status
    pub status: ContainerStatus,

    /// Container config
    pub config: ContainerConfig,
}

impl ContainerHandle {
    /// Create a new container handle.
    pub fn new(name: String, config: ContainerConfig) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            pid: None,
            status: ContainerStatus::Created,
            config,
        }
    }

    /// Set the process ID.
    pub fn with_pid(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }

    /// Set the status.
    pub fn with_status(mut self, status: ContainerStatus) -> Self {
        self.status = status;
        self
    }

    /// Get the container root directory.
    pub fn root_dir(&self) -> PathBuf {
        crate::rootfs::get_container_root().join(&self.name)
    }

    /// Get the rootfs path for this container (overlay merged view).
    pub fn rootfs_path(&self) -> PathBuf {
        crate::rootfs::get_container_root()
            .join(&self.name)
            .join(crate::rootfs::ROOTFS_DIR)
    }

    /// Get the upper layer path (writable layer).
    pub fn upper_path(&self) -> PathBuf {
        crate::rootfs::get_container_root()
            .join(&self.name)
            .join(crate::rootfs::UPPER_DIR)
    }

    /// Get the cgroup path for this container.
    pub fn cgroup_path(&self) -> PathBuf {
        PathBuf::from(crate::cgroup::CGROUP_ROOT)
            .join(crate::cgroup::CONTAINMENT_CGROUP)
            .join(&self.name)
    }

    /// Check if container has existing writable layer.
    pub fn has_existing_upper(&self) -> bool {
        rootfs::has_existing_upper(&self.name)
    }

    /// Get the size of the writable layer.
    pub fn upper_layer_size(&self) -> Result<u64> {
        rootfs::get_upper_layer_size(&self.name)
    }
}

/// Container status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerStatus {
    Created,
    Running,
    Paused,
    Stopped,
    Removing,
    Exited(i32),
}

impl std::fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerStatus::Created => write!(f, "created"),
            ContainerStatus::Running => write!(f, "running"),
            ContainerStatus::Paused => write!(f, "paused"),
            ContainerStatus::Stopped => write!(f, "stopped"),
            ContainerStatus::Removing => write!(f, "removing"),
            ContainerStatus::Exited(code) => write!(f, "exited ({})", code),
        }
    }
}

/// Container runtime - manages the lifecycle of containers.
#[derive(Debug)]
pub struct Container {
    handle: ContainerHandle,
    process: Option<ContainerProcess>,
    cgroup_manager: Option<CgroupManager>,
}

impl Container {
    /// Create a new container from the given configuration.
    pub fn new(config: ContainerConfig) -> Result<Self> {
        let name = config.name.clone();
        let handle = ContainerHandle::new(name, config);

        // Create container directory structure
        let root_dir = handle.root_dir();
        std::fs::create_dir_all(root_dir.join("fs"))?;
        std::fs::create_dir_all(root_dir.join("config"))?;

        // Save config to file for persistence
        let config_path = root_dir.join("config").join("config.json");
        let config_json = serde_json::to_string_pretty(&handle.config)?;
        std::fs::write(config_path, config_json)?;

        Ok(Self {
            handle,
            process: None,
            cgroup_manager: None,
        })
    }

    /// Start the container.
    pub fn start(&mut self) -> Result<()> {
        if self.process.is_some() {
            anyhow::bail!("Container already started");
        }

        // Initialize cgroup manager
        let mut cgroup_mgr = CgroupManager::new(&self.handle.config.name)?;
        cgroup_mgr.initialize()?;

        // Apply resource limits from config
        self.apply_resource_limits(&cgroup_mgr)?;

        // Spawn the container process
        let process = ContainerProcess::spawn(&self.handle.config)?;

        // Add the process to the cgroup
        #[cfg(target_os = "linux")]
        {
            cgroup_mgr.add_process(process.pid)?;
        }

        self.handle.status = ContainerStatus::Running;
        #[cfg(target_os = "linux")]
        {
            self.handle.pid = Some(process.pid.as_raw() as u32);
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.handle.pid = Some(process.pid);
        }
        self.process = Some(process);
        self.cgroup_manager = Some(cgroup_mgr);

        // Set up bridge networking for Linux bridge mode.
        #[cfg(target_os = "linux")]
        {
            use crate::config::NetworkMode;
            let mode = self.handle.config.network.mode_enum();
            if mode == NetworkMode::Bridge || mode == NetworkMode::None {
                let state_dir = crate::rootfs::get_container_root();
                let mut network_state = crate::network::NetworkState::default();
                network_state.mode = mode.as_str().to_string();

                if mode == NetworkMode::Bridge {
                    if let Some(pid) = self.handle.pid {
                        match crate::network::setup_bridge_for_container(
                            &state_dir,
                            &self.handle.name,
                            pid,
                        ) {
                            Ok(state) => {
                                network_state = state;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Bridge setup failed for {}: {}",
                                    self.handle.name,
                                    e
                                );
                            }
                        }
                    }
                }

                // Inject /etc/resolv.conf and /etc/hosts into the rootfs.
                let rootfs_path = self.handle.rootfs_path();
                let container_ip = network_state.container_ip.as_deref();
                let hosts_entries = crate::network::read_hosts_entries(&state_dir);
                let dns: Vec<String> = self
                    .handle
                    .config
                    .network
                    .dns
                    .clone()
                    .unwrap_or_default();
                let dns_refs: Vec<&str> = dns.iter().map(|s| s.as_str()).collect();
                if let Err(e) = crate::rootfs::inject_network_files(
                    &self.handle.config.hostname,
                    container_ip,
                    &dns_refs,
                    &hosts_entries,
                ) {
                    tracing::warn!(
                        "Failed to inject network files into {}: {}",
                        rootfs_path.display(),
                        e
                    );
                }

                // Persist network state for teardown.
                if let Ok(manager) = ContainerManager::new() {
                    if let Ok(Some(mut metadata)) = manager.find(&self.handle.name) {
                        metadata.network_state = network_state.clone();
                        let _ = manager.save(&metadata);
                    }
                }

                // Set up nftables port forwarding for bridge mode.
                if mode == NetworkMode::Bridge
                    && !self.handle.config.network.port_mappings.is_empty()
                {
                    if let Some(ip) = network_state.container_ip.as_deref() {
                        let mappings: Vec<(u16, u16)> = self
                            .handle
                            .config
                            .network
                            .port_mappings
                            .iter()
                            .map(|pm| (pm.host_port, pm.container_port))
                            .collect();
                        match crate::network::NftablesPortForwarder::new(
                            &self.handle.name,
                            ip,
                            &mappings,
                        ) {
                            Ok(fw) => {
                                if let Ok(manager) = ContainerManager::new() {
                                    if let Ok(Some(mut metadata)) =
                                        manager.find(&self.handle.name)
                                    {
                                        metadata.network_state.nft_table =
                                            Some(fw.table_name.clone());
                                        let _ = manager.save(&metadata);
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "nftables port forwarding failed for {}: {}",
                                    self.handle.name,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }

        // Spawn a background healthcheck task if configured.
        if let Some(health) = self.handle.config.healthcheck.clone() {
            if let Some(pid) = self.handle.pid {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let name = self.handle.name.clone();
                    let runner = HealthcheckRunner::new(name, pid, health);
                    handle.spawn(runner.run());
                    tracing::info!(
                        "Spawned healthcheck task for container {}",
                        self.handle.name
                    );
                } else {
                    tracing::debug!(
                        "No tokio runtime available; skipping background healthcheck for {}",
                        self.handle.name
                    );
                }
            }
        }

        tracing::info!("Container {} started", self.handle.name);

        Ok(())
    }

    /// Apply resource limits from config to cgroup.
    fn apply_resource_limits(&self, cgroup: &CgroupManager) -> Result<()> {
        use crate::cgroup::{self};
        use crate::config;

        // Memory limit
        if let Some(ref memory_str) = self.handle.config.resources.memory {
            let bytes = config::parse_size(memory_str)?;
            cgroup.set_memory_limit(bytes)?;
            tracing::debug!("Set memory limit: {} bytes", bytes);
        }

        // Memory swap limit
        if let Some(ref swap_str) = self.handle.config.resources.memory_swap {
            let bytes = config::parse_size(swap_str)?;
            cgroup.set_memory_swap_limit(bytes)?;
        }

        // CPU limit
        if let Some(ref cpu_str) = self.handle.config.resources.cpu {
            let cpu_count: f64 = cpu_str.parse()
                .or_else(|_| {
                    let s = cpu_str.trim().trim_end_matches('%');
                    s.parse::<f64>().map(|p| p / 100.0)
                })
                .unwrap_or(1.0);

            let (quota, period) = cgroup::cpu_count_to_quota(cpu_count);
            cgroup.set_cpu_limit(quota, period)?;
            tracing::debug!("Set CPU limit: {} us / {} us", quota, period);
        }

        // CPU affinity
        if let Some(ref cpus) = self.handle.config.resources.cpus {
            cgroup.set_cpu_affinity(cpus)?;
        }

        // CPU shares
        if let Some(shares) = self.handle.config.resources.cpu_shares {
            cgroup.set_cpu_shares(shares)?;
        }

        // PIDs limit
        if let Some(limit) = self.handle.config.resources.pids_limit {
            cgroup.set_pids_limit(limit)?;
            tracing::debug!("Set PIDs limit: {}", limit);
        }

        Ok(())
    }

    /// Stop the container (send SIGTERM).
    pub fn stop(&mut self) -> Result<()> {
        let process = self.process.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Container not running"))?;

        process.terminate()?;

        // Wait for process to exit
        let _ = process.wait();

        self.handle.status = ContainerStatus::Stopped;

        // Clean up bridge networking on Linux.
        #[cfg(target_os = "linux")]
        {
            Self::cleanup_networking(&self.handle.name);
        }

        // Clean up cgroup
        if let Some(cgroup) = self.cgroup_manager.take() {
            let _ = cgroup.destroy();
        }

        // Unmount overlay but keep upper layer for persistence
        rootfs::cleanup_overlay_mount(&self.handle.config)?;

        self.process = None;

        tracing::info!("Container {} stopped (upper layer preserved)", self.handle.name);

        Ok(())
    }

    /// Kill the container (send SIGKILL).
    pub fn kill(&mut self) -> Result<()> {
        let process = self.process.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Container not running"))?;

        process.kill_hard()?;

        // Wait for process to exit
        let _ = process.wait();

        self.handle.status = ContainerStatus::Stopped;

        // Clean up bridge networking on Linux.
        #[cfg(target_os = "linux")]
        {
            Self::cleanup_networking(&self.handle.name);
        }

        // Clean up cgroup
        if let Some(cgroup) = self.cgroup_manager.take() {
            let _ = cgroup.destroy();
        }

        // Unmount overlay but keep upper layer for persistence
        rootfs::cleanup_overlay_mount(&self.handle.config)?;

        self.process = None;

        tracing::info!("Container {} killed (upper layer preserved)", self.handle.name);

        Ok(())
    }

    /// Wait for the container to exit.
    pub fn wait(&self) -> Result<ContainerStatus> {
        if let Some(process) = &self.process {
            let state = process.wait()?;
            match state {
                crate::process::ProcessState::Exited(code) => {
                    Ok(ContainerStatus::Exited(code))
                }
                crate::process::ProcessState::Failed(code) => {
                    Ok(ContainerStatus::Exited(code))
                }
                crate::process::ProcessState::Running => {
                    Ok(ContainerStatus::Running)
                }
            }
        } else {
            Ok(self.handle.status)
        }
    }

    /// Get the container handle.
    pub fn handle(&self) -> &ContainerHandle {
        &self.handle
    }

    /// Check if the container is running.
    pub fn is_running(&self) -> bool {
        matches!(self.handle.status, ContainerStatus::Running)
    }

    /// Clean up networking artifacts (bridge + nftables) for a container.
    #[cfg(target_os = "linux")]
    fn cleanup_networking(name: &str) {
        let state_dir = crate::rootfs::get_container_root();
        if let Ok(manager) = ContainerManager::new() {
            if let Ok(Some(metadata)) = manager.find(name) {
                if !metadata.network_state.is_empty() {
                    let _ = crate::network::teardown_bridge_network(
                        &state_dir,
                        name,
                        &metadata.network_state,
                    );
                }
                if let Some(table) = metadata.network_state.nft_table.as_deref() {
                    let ruleset = format!("delete table ip {}\n", table);
                    let _ = std::process::Command::new("nft")
                        .args(["-f", "-"])
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .and_then(|mut child| {
                            use std::io::Write;
                            if let Some(mut stdin) = child.stdin.take() {
                                let _ = stdin.write_all(ruleset.as_bytes());
                            }
                            child.wait()
                        });
                }
            }
        }
    }

    /// Get current resource usage statistics.
    pub fn stats(&self) -> Result<ContainerStats> {
        if !self.is_running() || self.cgroup_manager.is_none() {
            return Ok(ContainerStats::default());
        }

        let cgroup = self.cgroup_manager.as_ref().unwrap();
        let (io_rbytes, io_wbytes) = cgroup.get_io_stats().unwrap_or((0, 0));
        let (cpu_periods, cpu_throttled, cpu_throttled_usec) =
            cgroup.get_cpu_throttling().unwrap_or((0, 0, 0));

        Ok(ContainerStats {
            memory_usage: Some(cgroup.get_memory_usage()?),
            memory_limit: cgroup.get_memory_limit()?,
            cpu_usage: Some(cgroup.get_cpu_usage()?),
            pids: cgroup.get_processes()?.len() as u64,
            io_rbytes,
            io_wbytes,
            cpu_periods,
            cpu_throttled,
            cpu_throttled_usec,
        })
    }

    /// Execute a command in the running container.
    pub fn exec(&self, command: &[String]) -> Result<()> {
        if !self.is_running() {
            anyhow::bail!("Container not running");
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(pid) = self.handle.pid {
                let pid = Pid::from_raw(pid as i32);

                // Enter container namespaces
                enter_container_namespaces(pid)?;

                // Execute the command
                // In a real implementation, this would fork and exec
                tracing::info!("Executing command in container {}: {:?}", self.handle.name, command);
            }
        }

        Ok(())
    }

    /// Remove the container (clean up resources).
    ///
    /// This permanently removes the container including all its writable layer changes.
    pub fn remove(&mut self) -> Result<()> {
        if self.is_running() {
            self.stop()?;
        }

        // Clean up rootfs including upper layer (full cleanup)
        rootfs::cleanup_rootfs(&self.handle.config)?;

        // Remove container root directory if it still exists
        let root_dir = self.handle.root_dir();
        if root_dir.exists() {
            std::fs::remove_dir_all(root_dir)?;
        }

        self.handle.status = ContainerStatus::Removing;

        tracing::info!("Container {} removed (upper layer deleted)", self.handle.name);

        Ok(())
    }
}

/// Container resource usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStats {
    /// Current memory usage in bytes
    pub memory_usage: Option<u64>,

    /// Memory limit in bytes (None = unlimited)
    pub memory_limit: Option<u64>,

    /// CPU usage in nanoseconds
    pub cpu_usage: Option<u64>,

    /// Number of processes
    pub pids: u64,

    /// Cumulative I/O read bytes
    pub io_rbytes: u64,
    /// Cumulative I/O write bytes
    pub io_wbytes: u64,

    /// CPU throttle periods
    pub cpu_periods: u64,
    /// Periods in which the cgroup was throttled
    pub cpu_throttled: u64,
    /// Time the cgroup was throttled, in microseconds
    pub cpu_throttled_usec: u64,
}

impl Default for ContainerStats {
    fn default() -> Self {
        Self {
            memory_usage: None,
            memory_limit: None,
            cpu_usage: None,
            pids: 0,
            io_rbytes: 0,
            io_wbytes: 0,
            cpu_periods: 0,
            cpu_throttled: 0,
            cpu_throttled_usec: 0,
        }
    }
}

impl Container {
    /// Pause the container (freeze cgroup).
    #[cfg(target_os = "linux")]
    pub fn pause(&mut self) -> Result<()> {
        if !self.is_running() {
            anyhow::bail!("Container not running");
        }

        if let Some(cgroup) = &self.cgroup_manager {
            let freezer_path = cgroup.path().join("cgroup.freeze");
            if freezer_path.exists() {
                std::fs::write(freezer_path, "1")?;
                self.handle.status = ContainerStatus::Paused;
                tracing::info!("Container {} paused", self.handle.name);
            } else {
                anyhow::bail!("Freezer controller not available");
            }
        }

        Ok(())
    }

    /// Resume the container (unfreeze cgroup).
    #[cfg(target_os = "linux")]
    pub fn resume(&mut self) -> Result<()> {
        if self.handle.status != ContainerStatus::Paused {
            anyhow::bail!("Container not paused");
        }

        if let Some(cgroup) = &self.cgroup_manager {
            let freezer_path = cgroup.path().join("cgroup.freeze");
            if freezer_path.exists() {
                std::fs::write(freezer_path, "0")?;
                self.handle.status = ContainerStatus::Running;
                tracing::info!("Container {} resumed", self.handle.name);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ResourceConfig, NetworkConfig, Namespaces};

    fn test_config() -> ContainerConfig {
        ContainerConfig {
            name: "test-container".to_string(),
            image: "python:3.12".to_string(),
            workdir: "/app".into(),
            env: std::collections::HashMap::new(),
            user: "root".to_string(),
            command: vec!["sleep".to_string(), "10".to_string()],
            resources: ResourceConfig::default(),
            network: NetworkConfig::default(),
            mounts: vec![],
            gpu: None,
            namespaces: Namespaces::default(),
            hostname: "test".to_string(),
            privileged: false,
            readonly_rootfs: false,
            architecture: None,
            platform: None,
            restart_policy: Default::default(),
            healthcheck: None,
            overlay_lowerdirs: None,
        }
    }

    #[test]
    fn test_container_new() {
        let config = test_config();
        let container = Container::new(config);
        assert!(container.is_ok());

        let container = container.unwrap();
        assert_eq!(container.handle().name, "test-container");
        assert_eq!(container.handle().status, ContainerStatus::Created);
    }

    #[test]
    fn test_container_stats_default() {
        let stats = ContainerStats::default();
        assert!(stats.memory_usage.is_none());
        assert!(stats.cpu_usage.is_none());
        assert_eq!(stats.pids, 0);
    }

    #[test]
    fn test_handle_rootfs_path() {
        let config = test_config();
        let handle = ContainerHandle::new("my-container".to_string(), config);

        assert!(handle.rootfs_path().ends_with("my-container/rootfs"));
    }
}
