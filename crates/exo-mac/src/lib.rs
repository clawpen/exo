//! Native macOS backend for Exo.
//!
//! Exo's Linux backend uses namespaces, cgroups, seccomp and overlayfs. Those
//! primitives do not exist on Darwin, so this backend provides a native
//! process-based execution mode for macOS: Exo-compatible lifecycle metadata,
//! logging and CLI semantics around host processes, with explicit warnings for
//! Linux-only isolation features.

pub mod backend;
pub mod gpu;
pub mod paths;

pub use backend::{LogOptions, NativeMacBackend, RunOptions};
pub use gpu::{detect_gpus, gpu_environment, MacGpuInfo, MacGpuVendor};
pub use paths::PathTranslator;

/// macOS backend configuration.
#[derive(Debug, Clone)]
pub struct MacConfig {
    /// Human-readable backend name used in diagnostics.
    pub backend_name: String,
}

impl Default for MacConfig {
    fn default() -> Self {
        Self {
            backend_name: "native-macos".to_string(),
        }
    }
}
