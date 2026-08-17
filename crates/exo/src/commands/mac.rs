//! Shared helpers for macOS backend selection.

#[cfg(target_os = "macos")]
use anyhow::Result;

/// Backend selected for a macOS container lifecycle command.
///
/// `auto` intentionally resolves to the Linux microVM. Native host-process
/// execution remains available, but must be requested explicitly. This keeps
/// container-shaped commands fail-closed instead of silently running an image
/// label as a host process.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendSelection {
    Native,
    Linux,
}

#[cfg(target_os = "macos")]
pub fn select_backend(requested: &str) -> Result<BackendSelection> {
    let selected = if requested == "auto" {
        std::env::var("EXO_BACKEND").unwrap_or_else(|_| "linux".to_string())
    } else {
        requested.to_string()
    };

    match selected.trim().to_ascii_lowercase().as_str() {
        "native" => Ok(BackendSelection::Native),
        "linux" => Ok(BackendSelection::Linux),
        "auto" => Ok(BackendSelection::Linux),
        other => anyhow::bail!(
            "unsupported macOS backend '{}'; expected auto, native, or linux",
            other
        ),
    }
}

#[cfg(target_os = "macos")]
pub fn native_backend() -> Result<exo_mac::NativeMacBackend> {
    exo_mac::NativeMacBackend::new(exo_mac::MacConfig::default())
}

#[cfg(target_os = "macos")]
pub fn linux_backend() -> exo_vm_mac::MacLinuxBackend {
    exo_vm_mac::MacLinuxBackend::new(exo_vm_mac::VmConfig::load())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn explicit_backend_selection_is_strict() {
        assert_eq!(select_backend("native").unwrap(), BackendSelection::Native);
        assert_eq!(select_backend("linux").unwrap(), BackendSelection::Linux);
        assert!(select_backend("docker").is_err());
    }
}
