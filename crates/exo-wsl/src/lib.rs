//! WSL2 backend for Exo on Windows.
//!
//! This module handles all WSL2-specific operations when running on Windows.

#[cfg(windows)]
pub mod distro;
#[cfg(windows)]
pub mod command;
#[cfg(windows)]
pub mod mount;
#[cfg(windows)]
pub mod gpu;
#[cfg(windows)]
pub mod networking;
#[cfg(windows)]
pub mod deploy;
#[cfg(windows)]
pub mod paths;
#[cfg(windows)]
pub mod daemon_client;
#[cfg(windows)]
pub mod windows_networking;
#[cfg(windows)]
pub mod wsl_daemon;

#[cfg(windows)]
pub use distro::{WslDistro, WslDistroManager};
#[cfg(windows)]
pub use command::{WslCommand, WslResult};
#[cfg(windows)]
pub use mount::{WslMount, MountSpec};
#[cfg(windows)]
pub use gpu::{WslGpuDetector, GpuInfo, GpuVendor, WslGpuConfig};
#[cfg(windows)]
pub use networking::{NetworkManager, NetworkConfig, NetworkMode, PortMapping, PortProtocol, ContainerNetwork, DnsEntry, AgentNetworkConfig};
#[cfg(windows)]
pub use deploy::WslDeployer;
#[cfg(windows)]
pub use paths::PathTranslator;
#[cfg(windows)]
pub use daemon_client::{DaemonClient, ContainerSpec as DaemonContainerSpec, MountSpec as DaemonMountSpec, RunResult};
#[cfg(windows)]
pub use windows_networking::{WindowsPortForwarder, PortForwardingRule};

#[cfg(windows)]
use anyhow::Result;

/// Stub Result type for non-Windows platforms
#[cfg(not(windows))]
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// WSL2 backend configuration.
#[derive(Debug, Clone)]
pub struct WslConfig {
    /// Name of the WSL2 distro to use
    pub distro_name: String,

    /// Path where the distro is stored
    pub distro_path: String,

    /// Memory limit for WSL2 (in GB, none = unlimited)
    pub memory_limit: Option<u64>,

    /// Swap size (in GB)
    pub swap_size: Option<u64>,

    /// Enable GPU passthrough
    pub gpu_passthrough: bool,
}

impl Default for WslConfig {
    fn default() -> Self {
        Self {
            distro_name: "Ubuntu".to_string(),  // Use default Ubuntu distro
            distro_path: "%LOCALAPPDATA%\\exo\\wsl".to_string(),
            memory_limit: None,
            swap_size: Some(4),
            gpu_passthrough: true,
        }
    }
}

/// Check if WSL2 is installed and available.
#[cfg(windows)]
pub fn check_wsl_installed() -> Result<bool> {
    use std::process::Command;

    let output = Command::new("wsl")
        .args(["--status"])
        .output()?;

    Ok(output.status.success())
}

/// Stub for non-Windows platforms.
#[cfg(not(windows))]
pub fn check_wsl_installed() -> Result<bool> {
    Ok(false)
}

/// Get the WSL version (1 or 2).
#[cfg(windows)]
pub fn get_wsl_version() -> Result<u32> {
    use std::process::Command;

    let output = Command::new("wsl")
        .args(["--status"])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse "Default Version: 2"
    for line in stdout.lines() {
        if line.contains("Default Version") {
            if let Some(version) = line.split(':').nth(1) {
                return Ok(version.trim().parse().unwrap_or(1));
            }
        }
    }

    // Check WSL2 specific features
    if stdout.contains("WSL 2") || stdout.contains("Kernel") {
        return Ok(2);
    }

    Ok(1) // Default to WSL1
}

#[cfg(not(target_os = "windows"))]
pub fn get_wsl_version() -> Result<u32> {
    Ok(0)
}

#[cfg(test)]
mod tests {
    

    #[test]
    #[cfg(windows)]
    fn test_check_wsl() {
        // Just verify it doesn't panic
        let _installed = check_wsl_installed().ok();
        let _version = get_wsl_version().ok();
    }
}
