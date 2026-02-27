//! GPU passthrough support for WSL2.

use crate::WslConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Detected GPU information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub vendor: GpuVendor,
    pub name: String,
    pub memory_mb: Option<u64>,
    pub driver_version: Option<String>,
}

/// GPU vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

/// GPU detector for Windows.
pub struct WslGpuDetector {
    config: WslConfig,
}

impl WslGpuDetector {
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// Detect GPUs on Windows (host side).
    pub fn detect_host_gpus(&self) -> Result<Vec<GpuInfo>> {
        let mut gpus = vec![];

        // Try NVIDIA first
        if let Ok(nvidia_gpus) = self.detect_nvidia() {
            gpus.extend(nvidia_gpus);
        }

        // Then try AMD
        if let Ok(amd_gpus) = self.detect_amd() {
            gpus.extend(amd_gpus);
        }

        Ok(gpus)
    }

    /// Check if WSL2 supports GPU passthrough.
    pub fn check_wsl_gpu_support(&self) -> bool {
        // WSL2 with GPU support requires Windows 11 or Windows 10 with specific updates
        self.is_wsl2() && self.has_dxgl_or_wslg()
    }

    /// Get GPU devices visible inside WSL2.
    pub fn wsl_gpu_devices(&self) -> Result<Vec<GpuInfo>> {
        // Run detection inside WSL2
        let output = std::process::Command::new("wsl")
            .args([
                "--distribution",
                &self.config.distro_name,
                "--command",
                "ls /dev/dri 2>/dev/null || true",
            ])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut gpus = vec![];

        if stdout.contains("card") {
            // GPUs are available in WSL2
            // For NVIDIA, check nvidia-smi
            if let Ok(nvidia) = self.check_nvidia_in_wsl() {
                gpus.push(nvidia);
            }
        }

        Ok(gpus)
    }

    fn detect_nvidia(&self) -> Result<Vec<GpuInfo>> {
        let mut gpus = vec![];

        // Try nvidia-smi on Windows
        if let Ok(output) = std::process::Command::new("nvidia-smi")
            .args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader"])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                    if parts.len() >= 2 {
                        let memory_mb = parts[1].split_whitespace()
                            .next()
                            .and_then(|s| s.parse::<u64>().ok());

                        gpus.push(GpuInfo {
                            vendor: GpuVendor::Nvidia,
                            name: parts[0].to_string(),
                            memory_mb,
                            driver_version: parts.get(2).map(|s| s.to_string()),
                        });
                    }
                }
            }
        }

        Ok(gpus)
    }

    fn detect_amd(&self) -> Result<Vec<GpuInfo>> {
        // AMD GPU detection on Windows is more complex
        // Use WMIC or PowerShell
        Ok(vec![])
    }

    fn check_nvidia_in_wsl(&self) -> Result<GpuInfo> {
        let output = std::process::Command::new("wsl")
            .args([
                "--distribution",
                &self.config.distro_name,
                "--command",
                "nvidia-smi --query-gpu=name,memory.total --format=csv,noheader 2>/dev/null || true",
            ])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let first_line = stdout.lines().next();

        if let Some(line) = first_line {
            let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
            let memory_mb = parts.get(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<u64>().ok());

            return Ok(GpuInfo {
                vendor: GpuVendor::Nvidia,
                name: parts.first().unwrap_or(&"NVIDIA GPU").to_string(),
                memory_mb,
                driver_version: None,
            });
        }

        anyhow::bail!("No NVIDIA GPU found in WSL2")
    }

    fn is_wsl2(&self) -> bool {
        // Check if we're running under WSL2
        if let Ok(output) = std::process::Command::new("wsl")
            .args(["--status"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.contains("Default Version: 2") || stdout.contains("WSL 2")
        } else {
            false
        }
    }

    fn has_dxgl_or_wslg(&self) -> bool {
        // Check for WSLg (Windows Subsystem for Linux GUI)
        // which includes GPU support
        std::path::Path::new(r"C:\Windows\System32\wslg.exe").exists()
    }
}

/// Configure GPU passthrough for a container.
pub struct WslGpuConfig {
    pub enabled: bool,
    pub vendor: GpuVendor,
    pub devices: Vec<String>, // e.g., ["0", "1"] or ["all"]
}

impl WslGpuConfig {
    pub fn to_env_vars(&self) -> Vec<(&'static str, String)> {
        let mut vars = vec![];

        match self.vendor {
            GpuVendor::Nvidia => {
                vars.push(("NVIDIA_VISIBLE_DEVICES", self.devices.join(",")));
                vars.push(("NVIDIA_DRIVER_CAPABILITIES", "compute,utility".to_string()));
            }
            GpuVendor::Amd => {
                vars.push(("ROCR_VISIBLE_DEVICES", self.devices.join(",")));
            }
            _ => {}
        }

        vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_detect_gpus() {
        let detector = WslGpuDetector::new(WslConfig::default());
        // Just verify it doesn't crash
        let _gpus = detector.detect_host_gpus();
    }

    #[test]
    fn test_gpu_config_env() {
        let config = WslGpuConfig {
            enabled: true,
            vendor: GpuVendor::Nvidia,
            devices: vec!["0".to_string(), "1".to_string()],
        };

        let env = config.to_env_vars();
        assert_eq!(env[0].0, "NVIDIA_VISIBLE_DEVICES");
        assert_eq!(env[0].1, "0,1");
    }
}
