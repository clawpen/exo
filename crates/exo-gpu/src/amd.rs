//! AMD GPU (ROCm) support.

use crate::detector::GpuInfo;
use crate::GpuType;
use anyhow::Result;
use std::fs;
use std::path::Path;

/// Detect AMD GPUs (ROCm).
pub fn detect_amd_gpus() -> Result<Vec<GpuInfo>> {
    let mut gpus = vec![];

    // Check for AMD GPU presence via /dev/kfd
    if !Path::new("/dev/kfd").exists() {
        return Ok(gpus);
    }

    // Try to read GPU info from sysfs
    let drm_path = Path::new("/sys/class/drm");

    if let Ok(entries) = fs::read_dir(drm_path) {
        for entry in entries.flatten() {
            let path = entry.path();

            // Look for card* directories (AMD GPU devices)
            let card_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !card_name.starts_with("card") || card_name.contains('-') {
                continue;
            }

            // Check if this is an AMD GPU
            let device_path = path.join("device");
            if !device_path.exists() {
                continue;
            }

            // Try to read vendor ID (AMD is 0x1002)
            let vendor_path = device_path.join("vendor");
            if let Ok(vendor) = fs::read_to_string(&vendor_path) {
                if !vendor.contains("0x1002") && !vendor.contains("1002") {
                    continue;
                }
            } else {
                continue;
            }

            // Get GPU name
            let name = read_gpu_name(&device_path).unwrap_or_else(|| "AMD GPU".to_string());

            // Get memory info
            let memory = read_gpu_memory(&device_path).ok();

            // Get PCI ID
            let pci_id = read_pci_id(&device_path).ok();

            // Extract card number
            let card_num = card_name.strip_prefix("card").unwrap_or("0");

            let device_paths = vec![
                format!("/dev/dri/{}", card_name),
                format!("/dev/dri/renderD{}", card_num), // renderD128, etc.
                "/dev/kfd".to_string(),
            ];

            let library_paths = vec!["/opt/rocm/lib".to_string(), "/opt/rocm/hip/lib".to_string()];

            gpus.push(GpuInfo {
                gpu_type: GpuType::Amd,
                id: card_num.to_string(),
                name,
                memory_mb: memory,
                pci_id,
                device_paths,
                library_paths,
            });
        }
    }

    Ok(gpus)
}

fn read_gpu_name(device_path: &Path) -> Option<String> {
    let _uevent_path = device_path.join("uevent");
    // For now, return a generic name
    Some("AMD GPU".to_string())
}

fn read_gpu_memory(device_path: &Path) -> Result<u64> {
    // Try to read memory info from various sysfs locations
    let mem_info_path = device_path.join("mem_info_vram_total");
    if let Ok(mem_str) = fs::read_to_string(&mem_info_path) {
        let mem_bytes: u64 = mem_str.trim().parse()?;
        return Ok(mem_bytes / 1024 / 1024); // Convert to MB
    }

    // Alternative location for some GPUs
    let fb_path = device_path.join("gem_mem_info_fb");
    if let Ok(fb_str) = fs::read_to_string(&fb_path) {
        let mem_bytes: u64 = fb_str.trim().parse()?;
        return Ok(mem_bytes / 1024 / 1024);
    }

    anyhow::bail!("Could not read GPU memory")
}

fn read_pci_id(device_path: &Path) -> Result<String> {
    let uevent_path = device_path.join("uevent");
    let content = fs::read_to_string(&uevent_path)?;

    for line in content.lines() {
        if line.starts_with("PCI_SLOT_NAME=") {
            if let Some(slot) = line.strip_prefix("PCI_SLOT_NAME=") {
                return Ok(slot.to_string());
            }
        }
    }

    anyhow::bail!("Could not read PCI ID")
}

/// AMD GPU manager for container runtime.
#[derive(Debug)]
pub struct AmdGpu {
    available: bool,
}

impl AmdGpu {
    /// Create a new AMD GPU manager.
    pub fn new() -> Self {
        let available = std::path::Path::new("/dev/kfd").exists();
        Self { available }
    }

    /// Check if AMD GPUs are available.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Get ROCm version if available.
    pub fn rocm_version(&self) -> Option<String> {
        // Try reading from rocminfo or rocm_smi
        if let Ok(output) = std::process::Command::new("rocm_smi")
            .arg("--showproductname")
            .output()
        {
            if output.status.success() {
                return Some(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }
        None
    }
}

impl Default for AmdGpu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amd_gpu_new() {
        let gpu = AmdGpu::new();
        // Just verify it doesn't panic
        let _available = gpu.is_available();
    }
}
