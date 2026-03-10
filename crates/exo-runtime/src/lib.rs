//! Exo - Agent Container Runtime
//!
//! Core container runtime for AI agents from Claw Pen.
//!
//! # About Exo
//!
//! Exo provides lightweight, secure container isolation specifically designed
//! for AI agent workloads. Unlike traditional container runtimes built for
//! microservices, Exo is optimized for:
//!
//! - **Agent communication**: Stdio-based protocol instead of HTTP
//! - **Tool sandboxing**: Per-tool security contexts
//! - **Fast spawning**: Daemonless architecture for quick agent tasks
//! - **Rootless operation**: User namespaces for privilege separation
//!
//! # Example
//!
//! ```no_run
//! use exo_runtime::{Container, ContainerConfig};
//!
//! let config = ContainerConfig {
//!     name: "my-agent".to_string(),
//!     image: "python:3.12".to_string(),
//!     command: vec!["python".to_string()],
//!     ..Default::default()
//! };
//!
//! let container = Container::spawn(config)?;
//! ```

pub mod namespace;
pub mod process;
pub mod container;
pub mod config;
pub mod userns;
pub mod rootfs;
pub mod cgroup;
pub mod security;
pub mod seccomp;
pub mod binfmt;
pub mod storage;
pub mod image;
pub mod channel;
pub mod agent;
pub mod manager;

pub use container::{Container, ContainerHandle, ContainerStatus};
pub use manager::{ContainerManager, ContainerMetadata, ContainerJson, ContainerListJson, CONTAINER_STATE_DIR};
pub use config::{ContainerConfig, ResourceConfig, NetworkConfig, MountConfig};
pub use namespace::Namespace;
pub use userns::{UidMap, GidMap, setup_user_namespace};
pub use cgroup::{CgroupManager, parse_size as parse_cgroup_size, cpu_count_to_quota};
pub use security::{Capability, drop_capabilities, raise_capabilities, get_default_caps};
pub use seccomp::{SeccompProfile, SeccompAction, apply_seccomp, default_profile};
pub use binfmt::{Architecture, register_binfmt, is_qemu_available, setup_foreign_exec};
pub use storage::{OverlayfsDriver, ContainerOverlay};
pub use image::{ImageManager, OciManifest, StoredImage, TagOrDigest};
pub use channel::{AgentChannel, AgentMessage, ToolRequest, ToolResponse};
pub use agent::{AgentProfile, NetworkAccess, get_agent_profile, AgentConfigExt};

use anyhow::Result;

/// Result type for container operations.
pub type ContainerResult<T> = Result<T, ContainerError>;

/// Errors that can occur during container operations.
#[derive(thiserror::Error, Debug)]
pub enum ContainerError {
    #[error("namespace error: {0}")]
    Namespace(String),

    #[error("cgroup error: {0}")]
    Cgroup(String),

    #[error("process error: {0}")]
    Process(String),

    #[error("mount error: {0}")]
    Mount(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("other: {0}")]
    Other(String),
}

/// Linux capabilities that can be dropped/kept.
#[derive(Debug, Clone, Copy)]
pub struct LinuxCapability(pub &'static str);

impl LinuxCapability {
    pub const CAP_NET_RAW: LinuxCapability = LinuxCapability("CAP_NET_RAW");
    pub const CAP_NET_ADMIN: LinuxCapability = LinuxCapability("CAP_NET_ADMIN");
    pub const CAP_SYS_ADMIN: LinuxCapability = LinuxCapability("CAP_SYS_ADMIN");
    pub const CAP_SYS_CHROOT: LinuxCapability = LinuxCapability("CAP_SYS_CHROOT");
}

/// Default capabilities to drop for containers.
pub const DEFAULT_CAPS_TO_DROP: &[LinuxCapability] = &[
    LinuxCapability::CAP_NET_RAW,
    LinuxCapability::CAP_NET_ADMIN,
    LinuxCapability::CAP_SYS_ADMIN,
    LinuxCapability::CAP_SYS_CHROOT,
];
