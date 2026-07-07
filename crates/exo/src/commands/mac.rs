//! Shared helpers for the native macOS backend.

#[cfg(target_os = "macos")]
use anyhow::Result;

#[cfg(target_os = "macos")]
pub fn backend() -> Result<exo_mac::NativeMacBackend> {
    exo_mac::NativeMacBackend::new(exo_mac::MacConfig::default())
}
