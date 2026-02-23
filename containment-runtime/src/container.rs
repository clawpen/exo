//! Container implementation using Linux kernel features.

use crate::cgroup::CgroupManager;
use crate::mount::MountSetup;
use crate::rootfs::Rootfs;
use crate::process::ContainerProcess;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tracing::info;

/// Container specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub id: Option<String>,
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub workdir: String,
    pub env: Vec<String>,
    pub mounts: Vec<MountSpec>,
    pub gpu: bool,
    pub memory_mb: Option<u64>,
    pub cpu_shares: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

/// Running container.
pub struct Container {
    id: String,
    spec: ContainerSpec,
    process: Option<ContainerProcess>,
    cgroup: Option<CgroupManager>,
}

impl Container {
    /// Create a new container from a spec.
    pub fn from_spec(spec: serde_json::Value) -> Result<Self> {
        let spec: ContainerSpec = serde_json::from_value(spec)?;

        let id = spec.id.clone().unwrap_or_else(|| {
            Uuid::new_v4().to_string()
        });

        Ok(Self {
            id,
            spec,
            process: None,
            cgroup: None,
        })
    }

    /// Get the container ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Start the container.
    pub fn start(&mut self) -> Result<()> {
        info!("Starting container {}", self.id);

        // Create container state directory
        let state_dir = format!("/var/lib/openclaw/containers/{}", self.id);
        std::fs::create_dir_all(&state_dir)?;
        std::fs::create_dir_all(format!("{}/logs", state_dir))?;

        // Set up cgroups for resource limiting
        if self.spec.memory_mb.is_some() || self.spec.cpu_shares.is_some() {
            let mut cgroup = CgroupManager::new(&self.id)?;
            if let Some(memory_mb) = self.spec.memory_mb {
                cgroup.set_memory_limit(memory_mb * 1024 * 1024)?;
            }
            if let Some(shares) = self.spec.cpu_shares {
                cgroup.set_cpu_shares(shares)?;
            }
            self.cgroup = Some(cgroup);
        }

        // Prepare root filesystem
        let rootfs = Rootfs::new(&self.spec.image, &state_dir)?;
        let rootfs_path = rootfs.prepare()?;

        // Set up namespaces and spawn the container process
        self.process = Some(ContainerProcess::spawn(ContainerProcessConfig {
            container_id: self.id.clone(),
            rootfs: rootfs_path,
            command: self.spec.command.clone(),
            workdir: self.spec.workdir.clone(),
            env: self.spec.env.clone(),
            mounts: self.spec.mounts.clone(),
            gpu: self.spec.gpu,
        })?);

        info!("Container {} started with PID {:?}", self.id, self.process.as_ref().map(|p| p.pid()));
        Ok(())
    }

    /// Stop the container.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(process) = &self.process {
            info!("Stopping container {}", self.id);
            process.terminate()?;
            self.process = None;
        }

        if let Some(cgroup) = &self.cgroup {
            cgroup.delete()?;
            self.cgroup = None;
        }

        Ok(())
    }

    /// Wait for the container to exit.
    pub fn wait(&self) -> Result<i32> {
        if let Some(process) = &self.process {
            process.wait()
        } else {
            Ok(0)
        }
    }

    /// Get the container status.
    pub fn status(&self) -> ContainerStatus {
        if let Some(process) = &self.process {
            if process.is_running() {
                ContainerStatus::Running
            } else {
                ContainerStatus::Stopped
            }
        } else {
            ContainerStatus::Stopped
        }
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        // Clean up on drop
        let _ = self.stop();
    }
}

#[derive(Clone)]
pub struct ContainerProcessConfig {
    pub container_id: String,
    pub rootfs: String,
    pub command: Vec<String>,
    pub workdir: String,
    pub env: Vec<String>,
    pub mounts: Vec<MountSpec>,
    pub gpu: bool,
}

/// Container status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerStatus {
    Created,
    Running,
    Stopped,
    Paused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_from_spec() {
        let spec_json = serde_json::json!({
            "name": "test",
            "image": "ubuntu:22.04",
            "command": ["/bin/sh"],
            "workdir": "/app",
            "env": ["TEST=1"],
            "mounts": [],
            "gpu": false
        });

        let container = Container::from_spec(spec_json);
        assert!(container.is_ok());
        let container = container.unwrap();
        assert!(container.id().len() > 0);
    }
}
