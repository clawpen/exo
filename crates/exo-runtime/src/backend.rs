//! Backend abstraction shared by Exo frontends.
//!
//! This module is the M1 contract: platform-specific runtimes should move
//! behind this trait so CLI/API dispatch can be backend-aware without
//! duplicating Linux, Windows WSL, and macOS native logic in each command.

use crate::{ContainerConfig, ContainerMetadata};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Capabilities advertised by a backend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub linux_containers: bool,
    pub native_processes: bool,
    pub gpu: bool,
    pub metal: bool,
    pub cgroups: bool,
    pub namespaces: bool,
    pub seccomp: bool,
    pub overlayfs: bool,
    pub port_forwarding: bool,
    pub volume_mounts: bool,
    pub daemon: bool,
    pub rootless: bool,
}

impl BackendCapabilities {
    /// Capabilities for the current Linux runtime backend.
    pub fn linux_runtime() -> Self {
        Self {
            linux_containers: true,
            native_processes: false,
            gpu: true,
            metal: false,
            cgroups: true,
            namespaces: true,
            seccomp: true,
            overlayfs: true,
            port_forwarding: true,
            volume_mounts: true,
            daemon: true,
            rootless: true,
        }
    }

    /// Capabilities for the current macOS native process backend.
    pub fn native_macos() -> Self {
        Self {
            linux_containers: false,
            native_processes: true,
            gpu: true,
            metal: true,
            cgroups: false,
            namespaces: false,
            seccomp: false,
            overlayfs: false,
            port_forwarding: false,
            volume_mounts: true,
            daemon: false,
            rootless: true,
        }
    }

    /// Capabilities for the current Windows WSL2 backend.
    pub fn windows_wsl2() -> Self {
        Self {
            linux_containers: true,
            native_processes: false,
            gpu: true,
            metal: false,
            cgroups: true,
            namespaces: true,
            seccomp: true,
            overlayfs: true,
            port_forwarding: true,
            volume_mounts: true,
            daemon: true,
            rootless: true,
        }
    }

    /// Capabilities currently enforced by the Exo-managed Linux microVM backend
    /// on macOS.
    ///
    /// The VM provides a Linux kernel boundary and the guest runtime requires an
    /// isolated overlay rootfs. Namespace, cgroup, seccomp, host-volume, and port
    /// forwarding claims remain false until the guest actually enforces them.
    pub fn macos_linux_microvm() -> Self {
        Self {
            linux_containers: true,
            native_processes: false,
            gpu: false,
            metal: false,
            cgroups: false,
            namespaces: false,
            seccomp: false,
            overlayfs: true,
            port_forwarding: false,
            volume_mounts: false,
            daemon: true,
            rootless: false,
        }
    }
}

/// Output returned by `run`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunResult {
    pub id: Option<String>,
    pub name: String,
    pub message: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunOptions {
    pub detach: bool,
    pub rm: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListOptions {
    pub all: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartOptions {
    pub attach: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StopOptions {
    pub force: bool,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoveOptions {
    pub force: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogOptions {
    pub follow: bool,
    pub tail: usize,
    pub timestamps: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            follow: false,
            tail: 100,
            timestamps: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecOptions {
    pub user: Option<String>,
    pub interactive: bool,
    pub tty: bool,
}

/// Log content returned by a backend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogStream {
    pub content: String,
}

/// Formal backend trait for M1.
#[async_trait]
pub trait ExoBackend: Send + Sync {
    async fn run(&self, config: ContainerConfig, opts: RunOptions) -> Result<RunResult>;
    async fn list(&self, opts: ListOptions) -> Result<Vec<ContainerMetadata>>;
    async fn start(&self, id: &str, opts: StartOptions) -> Result<()>;
    async fn stop(&self, id: &str, opts: StopOptions) -> Result<()>;
    async fn remove(&self, id: &str, opts: RemoveOptions) -> Result<()>;
    async fn logs(&self, id: &str, opts: LogOptions) -> Result<LogStream>;
    async fn exec(&self, id: &str, command: Vec<String>, opts: ExecOptions) -> Result<i32>;
    fn capabilities(&self) -> BackendCapabilities;
}
