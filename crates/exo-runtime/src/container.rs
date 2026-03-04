//! Main container implementation.

use crate::config::ContainerConfig;
use crate::process::ContainerProcess;
use crate::cgroup::CgroupManager;
use crate::rootfs;
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
        PathBuf::from(format!("/var/lib/openclaw/containers/{}", self.id))
    }

    /// Get the rootfs path for this container.
    pub fn rootfs_path(&self) -> PathBuf {
        PathBuf::from(crate::rootfs::CONTAINER_ROOT_DIR)
            .join(&self.name)
            .join(crate::rootfs::ROOTFS_DIR)
    }

    /// Get the cgroup path for this container.
    pub fn cgroup_path(&self) -> PathBuf {
        PathBuf::from(crate::cgroup::CGROUP_ROOT)
            .join(crate::cgroup::CONTAINMENT_CGROUP)
            .join(&self.name)
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

        tracing::info!("Container {} started", self.handle.name);

        Ok(())
    }

    /// Apply resource limits from config to cgroup.
    fn apply_resource_limits(&self, cgroup: &CgroupManager) -> Result<()> {
        use crate::cgroup::{self, CgroupManager};
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

        // Clean up cgroup
        if let Some(cgroup) = self.cgroup_manager.take() {
            let _ = cgroup.destroy();
        }

        self.process = None;

        tracing::info!("Container {} stopped", self.handle.name);

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

        // Clean up cgroup
        if let Some(cgroup) = self.cgroup_manager.take() {
            let _ = cgroup.destroy();
        }

        self.process = None;

        tracing::info!("Container {} killed", self.handle.name);

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

    /// Get current resource usage statistics.
    pub fn stats(&self) -> Result<ContainerStats> {
        if !self.is_running() || self.cgroup_manager.is_none() {
            return Ok(ContainerStats::default());
        }

        let cgroup = self.cgroup_manager.as_ref().unwrap();

        Ok(ContainerStats {
            memory_usage: Some(cgroup.get_memory_usage()?),
            memory_limit: cgroup.get_memory_limit()?,
            cpu_usage: Some(cgroup.get_cpu_usage()?),
            pids: cgroup.get_processes()?.len() as u64,
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
    pub fn remove(&mut self) -> Result<()> {
        if self.is_running() {
            self.stop()?;
        }

        // Clean up rootfs
        rootfs::cleanup_rootfs(&self.handle.config)?;

        let root_dir = self.handle.root_dir();
        if root_dir.exists() {
            std::fs::remove_dir_all(root_dir)?;
        }

        self.handle.status = ContainerStatus::Removing;

        tracing::info!("Container {} removed", self.handle.name);

        Ok(())
    }
}

/// Container resource usage statistics.
#[derive(Debug, Clone)]
pub struct ContainerStats {
    /// Current memory usage in bytes
    pub memory_usage: Option<u64>,

    /// Memory limit in bytes (None = unlimited)
    pub memory_limit: Option<u64>,

    /// CPU usage in nanoseconds
    pub cpu_usage: Option<u64>,

    /// Number of processes
    pub pids: u64,
}

impl Default for ContainerStats {
    fn default() -> Self {
        Self {
            memory_usage: None,
            memory_limit: None,
            cpu_usage: None,
            pids: 0,
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
