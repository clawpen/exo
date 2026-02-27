//! GPU detection - identifies available GPUs on the system.

use crate::{nvidia, amd};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Type of GPU detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuType {
    /// Auto-detect (default)
    Auto,
    /// NVIDIA GPU
    Nvidia,
    /// AMD GPU (ROCm)
    Amd,
}

impl std::fmt::Display for GpuType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuType::Auto => write!(f, "auto"),
            GpuType::Nvidia => write!(f, "nvidia"),
            GpuType::Amd => write!(f, "amd"),
        }
    }
}

impl std::str::FromStr for GpuType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" | "" => Ok(GpuType::Auto),
            "nvidia" => Ok(GpuType::Nvidia),
            "amd" | "rocm" => Ok(GpuType::Amd),
            _ => Err(format!("Unknown GPU type: {}", s)),
        }
    }
}

/// Information about a detected GPU.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    /// GPU type (Nvidia, AMD)
    pub gpu_type: GpuType,
    /// GPU ID (e.g., "0", "1")
    pub id: String,
    /// GPU model name
    pub name: String,
    /// Memory in bytes
    pub memory_mb: Option<u64>,
    /// PCI bus ID
    pub pci_id: Option<String>,
    /// Device paths to mount
    pub device_paths: Vec<String>,
    /// Driver library paths
    pub library_paths: Vec<String>,
}

/// Detect all available GPUs on the system.
pub fn detect_gpus() -> anyhow::Result<Vec<GpuInfo>> {
    let mut gpus = vec![];

    // Try NVIDIA first
    if let Ok(nvidia_gpus) = nvidia::detect_nvidia_gpus() {
        gpus.extend(nvidia_gpus);
    }

    // Then try AMD
    if let Ok(amd_gpus) = amd::detect_amd_gpus() {
        gpus.extend(amd_gpus);
    }

    Ok(gpus)
}

/// Check if NVIDIA GPUs are available.
pub fn has_nvidia() -> bool {
    Path::new("/dev/nvidiactl").exists()
}

/// Check if AMD GPUs (ROCm) are available.
pub fn has_amd() -> bool {
    Path::new("/dev/kfd").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_type_from_str() {
        assert_eq!(GpuType::from_str("nvidia").unwrap(), GpuType::Nvidia);
        assert_eq!(GpuType::from_str("NVIDIA").unwrap(), GpuType::Nvidia);
        assert_eq!(GpuType::from_str("amd").unwrap(), GpuType::Amd);
        assert_eq!(GpuType::from_str("rocm").unwrap(), GpuType::Amd);
        assert_eq!(GpuType::from_str("").unwrap(), GpuType::Auto);
        assert!(GpuType::from_str("invalid").is_err());
    }
}
