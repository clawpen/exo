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
//! # fn main() -> anyhow::Result<()> {
//! use exo_runtime::{Container, ContainerConfig};
//!
//! let config = ContainerConfig {
//!     name: "my-agent".to_string(),
//!     image: "python:3.12".to_string(),
//!     command: vec!["python".to_string()],
//!     ..Default::default()
//! };
//!
//! let container = Container::new(config)?;
//! # Ok(())
//! # }
//! ```

pub mod agent;
pub mod backend;
pub mod binfmt;
pub mod cgroup;
pub mod channel;
pub mod config;
pub mod container;
pub mod error;
pub mod events;
pub mod image;
pub mod manager;
pub mod namespace;
pub mod network;
pub mod process;
pub mod reconcile;
pub mod rootfs;
pub mod seccomp;
pub mod secrets;
pub mod security;
pub mod storage;
pub mod userns;
pub mod volume;

pub use agent::{get_agent_profile, AgentConfigExt, AgentProfile, NetworkAccess};
pub use backend::{
    BackendCapabilities, ExecOptions, ExoBackend, ListOptions, LogOptions as BackendLogOptions,
    LogStream, RemoveOptions, RunOptions as BackendRunOptions, RunResult, StartOptions,
    StopOptions,
};
pub use binfmt::{is_qemu_available, register_binfmt, setup_foreign_exec, Architecture};
pub use cgroup::{cpu_count_to_quota, parse_size as parse_cgroup_size, CgroupManager};
pub use channel::{AgentChannel, AgentMessage, ToolRequest, ToolResponse};
pub use config::{BackendSelection, ContainerConfig, MountConfig, NetworkConfig, ResourceConfig};
pub use config::{RestartPolicy, SandboxMode};
pub use container::{Container, ContainerHandle, ContainerStatus};
pub use error::{
    envelope_for, exit_code_for, ErrorBody, ErrorEnvelope, ExoError, ExoResult, EXIT_BACKEND,
    EXIT_CONFLICT, EXIT_INTERNAL, EXIT_INVALID_INPUT, EXIT_NOT_FOUND, EXIT_OK,
};
pub use events::{Event, EventLog, EventType};
pub use image::{ImageManager, OciManifest, StoredImage, TagOrDigest};
pub use manager::{
    ContainerJson, ContainerListJson, ContainerManager, ContainerMetadata, CONTAINER_STATE_DIR,
};
pub use namespace::Namespace;
pub use reconcile::{ReconcileOptions, ReconcileSummary, Reconciler, DEFAULT_CGROUP_ROOT};
pub use seccomp::{apply_seccomp, default_profile, SeccompAction, SeccompProfile};
pub use secrets::SecretStore;
pub use security::{drop_capabilities, get_default_caps, raise_capabilities, Capability};
pub use storage::{ContainerOverlay, OverlayfsDriver};
pub use userns::{setup_user_namespace, GidMap, UidMap};
pub use volume::VolumeStore;

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
