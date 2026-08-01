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
mod oci;
mod paths;
mod vmm;

pub use backend::MacLinuxBackend;
pub use config::VmConfig;
pub use daemon::{VmDaemonClient, VmDaemonRequest, VmDaemonResponse};
pub use paths::{control_socket_path, daemon_log_path, guest_agent_binary_path};
pub use vmm::VmManager;

/// Verify that an `exo` binary is signed with the virtualization entitlement
/// required to boot VMs. Checks the current executable by default.
#[cfg(target_os = "macos")]
pub fn ensure_virtualization_entitlement() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let exe_str = exe.to_str().unwrap_or("");
    let output = std::process::Command::new("codesign")
        .args(["-dv", "--entitlements", "-", exe_str])
        .output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !text.contains("com.apple.security.virtualization") {
        anyhow::bail!(
            "the exo binary is not signed with the virtualization entitlement. \
             Run: scripts/sign-exo.sh {} \
             (or codesign --sign - --force --entitlements crates/exo-vm-mac/entitlements.plist {})",
            exe.display(),
            exe.display()
        );
    }
    Ok(())
}
