//! Main container implementation.

use crate::config::ContainerConfig;
use crate::process::ContainerProcess;
use crate::namespace::Namespace;
use anyhow::Result;
use std::path::PathBuf;
use uuid::Uuid;

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

        Ok(Self {
            handle,
            process: None,
        })
    }

    /// Start the container.
    pub fn start(&mut self) -> Result<()> {
        if self.process.is_some() {
            anyhow::bail!("Container already started");
        }

        // Spawn the container process
        let process = ContainerProcess::spawn(&self.handle.config)?;
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

        Ok(())
    }

    /// Stop the container (send SIGTERM).
    pub fn stop(&mut self) -> Result<()> {
        let process = self.process.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Container not running"))?;

        process.terminate()?;
        self.handle.status = ContainerStatus::Stopped;
        self.process = None;

        Ok(())
    }

    /// Kill the container (send SIGKILL).
    pub fn kill(&mut self) -> Result<()> {
        let process = self.process.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Container not running"))?;

        process.kill_hard()?;
        self.handle.status = ContainerStatus::Stopped;
        self.process = None;

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

    /// Execute a command in the running container.
    pub fn exec(&self, command: &[String]) -> Result<()> {
        if !self.is_running() {
            anyhow::bail!("Container not running");
        }

        // TODO: Implement exec by entering container namespaces
        // and executing the command

        tracing::info!("Executing command in container: {:?}", command);
        Ok(())
    }

    /// Remove the container (clean up resources).
    pub fn remove(&mut self) -> Result<()> {
        if self.is_running() {
            self.stop()?;
        }

        let root_dir = self.handle.root_dir();
        if root_dir.exists() {
            std::fs::remove_dir_all(root_dir)?;
        }

        self.handle.status = ContainerStatus::Removing;

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
}
