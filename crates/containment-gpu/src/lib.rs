//! GPU passthrough support for OpenClaw containers.
//!
//! Supports NVIDIA (via NVML) and AMD (via ROCm) GPUs.

mod detector;
mod nvidia;
mod amd;

pub use detector::{detect_gpus, GpuInfo, GpuType};
pub use nvidia::{NvidiaGpu, NvidiaDevice};
pub use amd::AmdGpu;

use anyhow::Result;

/// Configuration for GPU passthrough to a container.
#[derive(Debug, Clone)]
pub struct GpuConfig {
    /// Which GPU type to use (Auto, Nvidia, Amd)
    pub gpu_type: GpuType,
    /// Specific device IDs to passthrough (e.g., ["0", "1"] or ["all"])
    pub devices: Vec<String>,
    /// Whether to include all GPU devices
    pub all: bool,
    /// NVIDIA-specific: compute mode (exclusive, shared, etc.)
    pub compute_mode: Option<String>,
    /// Mount paths for GPU libraries
    pub library_paths: Vec<String>,
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            gpu_type: GpuType::Auto,
            devices: vec!["all".to_string()],
            all: true,
            compute_mode: None,
            library_paths: vec![],
        }
    }
}

impl GpuConfig {
    /// Create a new GPU config with all GPUs accessible.
    pub fn all() -> Self {
        Self::default()
    }

    /// Create a GPU config for specific devices.
    pub fn devices(devices: Vec<String>) -> Self {
        Self {
            devices,
            all: false,
            ..Default::default()
        }
    }

    /// Set the GPU type.
    pub fn with_gpu_type(mut self, gpu_type: GpuType) -> Self {
        self.gpu_type = gpu_type;
        self
    }

    /// Get the environment variables needed for GPU access in the container.
    pub fn environment_vars(&self) -> Vec<(&'static str, String)> {
        let mut vars = vec![];

        match self.gpu_type {
            GpuType::Nvidia => {
                vars.push(("NVIDIA_VISIBLE_DEVICES", self.devices.join(",")));
                vars.push(("NVIDIA_DRIVER_CAPABILITIES", "compute,utility".to_string()));
            }
            GpuType::Amd => {
                vars.push(("ROCR_VISIBLE_DEVICES", self.devices.join(",")));
                vars.push(("HIP_VISIBLE_DEVICES", self.devices.join(",")));
            }
            GpuType::Auto => {
                // Will be resolved at runtime
            }
        }

        vars
    }

    /// Get the device paths to mount into the container.
    pub fn device_paths(&self) -> Result<Vec<String>> {
        let gpus = detect_gpus()?;

        let paths: Vec<String> = gpus
            .iter()
            .filter(|gpu| {
                let type_matches = self.gpu_type == GpuType::Auto || self.gpu_type == gpu.gpu_type;
                if self.all {
                    type_matches
                } else {
                    (self.devices.contains(&gpu.id.to_string()) || self.devices.iter().any(|d| d == "all")) && type_matches
                }
            })
            .flat_map(|gpu| gpu.device_paths.clone())
            .collect();

        Ok(paths)
    }

    /// Validate that the requested GPUs are available.
    pub fn validate(&self) -> Result<()> {
        let available = detect_gpus()?;

        if available.is_empty() && (self.all || !self.devices.is_empty()) {
            anyhow::bail!("No GPUs detected on this system");
        }

        match self.gpu_type {
            GpuType::Nvidia => {
                if !available.iter().any(|g| matches!(g.gpu_type, GpuType::Nvidia)) {
                    anyhow::bail!("NVIDIA GPU requested but no NVIDIA devices found");
                }
            }
            GpuType::Amd => {
                if !available.iter().any(|g| matches!(g.gpu_type, GpuType::Amd)) {
                    anyhow::bail!("AMD GPU requested but no AMD devices found");
                }
            }
            GpuType::Auto => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_config_default() {
        let config = GpuConfig::default();
        assert!(config.all);
        assert_eq!(config.devices, vec!["all"]);
    }

    #[test]
    fn test_gpu_config_devices() {
        let config = GpuConfig::devices(vec!["0".to_string(), "1".to_string()]);
        assert!(!config.all);
        assert_eq!(config.devices.len(), 2);
    }
}
