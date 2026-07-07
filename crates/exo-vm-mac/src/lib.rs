//! Exo-managed Linux microVM backend for macOS.

pub mod bridge;
pub mod config;
pub mod daemon;
pub mod state;

mod agent_client;
mod backend;
mod builder;
mod ffi;
mod image;
mod paths;
mod vmm;

pub use backend::MacLinuxBackend;
pub use config::VmConfig;
pub use daemon::{VmDaemonClient, VmDaemonRequest, VmDaemonResponse};
pub use paths::{control_socket_path, daemon_log_path, guest_agent_binary_path};
pub use vmm::VmManager;
