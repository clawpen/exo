//! NVIDIA GPU support using NVML.

use crate::detector::GpuInfo;
use crate::GpuType;
use anyhow::Result;

/// Detect NVIDIA GPUs using NVML.
pub fn detect_nvidia_gpus() -> Result<Vec<GpuInfo>> {
    let mut gpus = vec![];

    // Try to initialize NVML
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            tracing::debug!("NVML initialization failed: {}", e);
            return Ok(gpus);
        }
    };

    let device_count = match nvml.device_count() {
        Ok(count) => count,
        Err(e) => {
            tracing::debug!("Failed to get device count: {}", e);
            return Ok(gpus);
        }
    };

    for i in 0..device_count {
        let device = match nvml.device_by_index(i) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let name = device
            .name()
            .unwrap_or_else(|_| "Unknown NVIDIA GPU".to_string());
        let memory = device.memory_info().ok().map(|m| m.total / 1024 / 1024);
        let pci_id = device.pci_info().ok().map(|p| p.bus_id);

        let mut device_paths = vec![
            format!("/dev/nvidia{}", i),
            "/dev/nvidiactl".to_string(),
            "/dev/nvidia-uvm".to_string(),
            "/dev/nvidia-uvm-tools".to_string(),
        ];

        // Remove duplicates while preserving order
        device_paths.sort();
        device_paths.dedup();

        let library_paths = vec![
            "/usr/lib/x86_64-linux-gnu/nvidia".to_string(),
            "/usr/lib/nvidia".to_string(),
        ];

        gpus.push(GpuInfo {
            gpu_type: GpuType::Nvidia,
            id: i.to_string(),
            name,
            memory_mb: memory,
            pci_id,
            device_paths,
            library_paths,
        });
    }

    Ok(gpus)
}

/// Represents a specific NVIDIA GPU device.
#[derive(Debug, Clone)]
pub struct NvidiaDevice {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub pci_id: String,
}

impl NvidiaDevice {
    pub fn from_nvml(device: &nvml_wrapper::Device<'_>, index: u32) -> Result<Self> {
        Ok(Self {
            index,
            name: device.name().unwrap_or_default(),
            uuid: device.uuid().unwrap_or_default(),
            pci_id: device.pci_info().ok().map(|p| p.bus_id).unwrap_or_default(),
        })
    }
}

/// NVIDIA GPU manager for container runtime.
#[derive(Debug)]
pub struct NvidiaGpu {
    nvml: Option<nvml_wrapper::Nvml>,
}

impl NvidiaGpu {
    /// Create a new NVIDIA GPU manager.
    pub fn new() -> Self {
        let nvml = nvml_wrapper::Nvml::init().ok();
        Self { nvml }
    }

    /// Check if NVIDIA is available.
    pub fn is_available(&self) -> bool {
        self.nvml.is_some()
    }

    /// Get all NVIDIA GPUs.
    pub fn gpus(&self) -> Vec<NvidiaDevice> {
        let nvml = match &self.nvml {
            Some(n) => n,
            None => return vec![],
        };

        let count = nvml.device_count().unwrap_or(0);
        let mut gpus = vec![];

        for i in 0..count {
            if let Ok(device) = nvml.device_by_index(i) {
                if let Ok(gpu) = NvidiaDevice::from_nvml(&device, i) {
                    gpus.push(gpu);
                }
            }
        }

        gpus
    }

    /// Get the CUDA version.
    pub fn cuda_version(&self) -> Option<u32> {
        self.nvml
            .as_ref()?
            .sys_cuda_driver_version()
            .ok()
            .map(|v| v as u32)
    }

    /// Get the driver version.
    pub fn driver_version(&self) -> Option<String> {
        self.nvml.as_ref()?.sys_driver_version().ok()
    }

    /// Set compute mode for a GPU.
    pub fn set_compute_mode(&self, index: u32, mode: ComputeMode) -> Result<()> {
        let nvml = self
            .nvml
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("NVML not initialized"))?;
        let mut device = nvml.device_by_index(index)?;
        device.set_compute_mode(mode.into())?;
        Ok(())
    }
}

impl Default for NvidiaGpu {
    fn default() -> Self {
        Self::new()
    }
}

/// NVIDIA compute mode.
#[derive(Debug, Clone, Copy)]
pub enum ComputeMode {
    Default,
    ExclusiveThread,
    ExclusiveProcess,
    Prohibited,
}

impl From<ComputeMode> for nvml_wrapper::enum_wrappers::device::ComputeMode {
    fn from(value: ComputeMode) -> Self {
        match value {
            ComputeMode::Default => nvml_wrapper::enum_wrappers::device::ComputeMode::Default,
            ComputeMode::ExclusiveThread => {
                nvml_wrapper::enum_wrappers::device::ComputeMode::ExclusiveThread
            }
            ComputeMode::ExclusiveProcess => {
                nvml_wrapper::enum_wrappers::device::ComputeMode::ExclusiveProcess
            }
            ComputeMode::Prohibited => nvml_wrapper::enum_wrappers::device::ComputeMode::Prohibited,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nvidia_gpu_new() {
        let gpu = NvidiaGpu::new();
        // Just verify it doesn't panic - actual GPU detection depends on hardware
        let _available = gpu.is_available();
    }
}
