//! Native macOS GPU detection.
//!
//! On the native macOS backend, workloads run as host processes and therefore
//! already share the host GPU. "GPU passthrough" here means detecting the Mac
//! GPU (Apple Silicon integrated, or a discrete/eGPU), confirming availability,
//! and exposing it to the workload via environment variables and metadata.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::process::Command;

/// GPU vendor as reported by macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MacGpuVendor {
    Apple,
    Amd,
    Nvidia,
    Intel,
    Unknown,
}

impl MacGpuVendor {
    fn from_profiler(vendor: &str, name: &str) -> Self {
        let vendor = vendor.to_lowercase();
        let name = name.to_lowercase();
        if vendor.contains("apple") || name.contains("apple") {
            MacGpuVendor::Apple
        } else if vendor.contains("amd") || vendor.contains("ati") || name.contains("radeon") {
            MacGpuVendor::Amd
        } else if vendor.contains("nvidia") || name.contains("geforce") || name.contains("quadro") {
            MacGpuVendor::Nvidia
        } else if vendor.contains("intel") || name.contains("intel") {
            MacGpuVendor::Intel
        } else {
            MacGpuVendor::Unknown
        }
    }

    /// Short lowercase identifier used in environment variables.
    pub fn as_str(&self) -> &'static str {
        match self {
            MacGpuVendor::Apple => "apple",
            MacGpuVendor::Amd => "amd",
            MacGpuVendor::Nvidia => "nvidia",
            MacGpuVendor::Intel => "intel",
            MacGpuVendor::Unknown => "unknown",
        }
    }
}

/// Information about a detected macOS GPU.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacGpuInfo {
    pub name: String,
    pub vendor: MacGpuVendor,
    /// Whether Metal is supported (Apple's GPU API).
    pub metal_supported: bool,
    /// GPU/core count when reported (Apple Silicon).
    pub cores: Option<u32>,
    /// Dedicated VRAM in megabytes when reported (discrete GPUs).
    pub vram_mb: Option<u64>,
    /// True for the built-in GPU.
    pub builtin: bool,
}

/// Detect GPUs on macOS using `system_profiler`.
pub fn detect_gpus() -> Result<Vec<MacGpuInfo>> {
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            tracing::debug!("system_profiler exited with status {:?}", o.status.code());
            return Ok(vec![]);
        }
        Err(e) => {
            tracing::debug!("system_profiler unavailable: {}", e);
            return Ok(vec![]);
        }
    };

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(parse_displays(&value))
}

fn parse_displays(value: &serde_json::Value) -> Vec<MacGpuInfo> {
    let mut gpus = vec![];

    let Some(items) = value.get("SPDisplaysDataType").and_then(|v| v.as_array()) else {
        return gpus;
    };

    for item in items {
        let name = item
            .get("sppci_model")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("_name").and_then(|v| v.as_str()))
            .unwrap_or("Unknown GPU")
            .to_string();

        let vendor_raw = item
            .get("spdisplays_vendor")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let vendor = MacGpuVendor::from_profiler(vendor_raw, &name);

        let metal_supported = item
            .get("spdisplays_metal")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("supported"))
            .unwrap_or(false);

        let cores = item
            .get("sppci_cores")
            .and_then(|v| v.as_str())
            .and_then(|s| s.trim().parse::<u32>().ok());

        let vram_mb = item
            .get("spdisplays_vram")
            .or_else(|| item.get("spdisplays_vram_shared"))
            .and_then(|v| v.as_str())
            .and_then(parse_vram_mb);

        let builtin = item
            .get("sppci_bus")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("builtin"))
            .unwrap_or(false);

        gpus.push(MacGpuInfo {
            name,
            vendor,
            metal_supported,
            cores,
            vram_mb,
            builtin,
        });
    }

    gpus
}

/// Parse a `system_profiler` VRAM string such as "8 GB" or "1536 MB".
fn parse_vram_mb(value: &str) -> Option<u64> {
    let value = value.trim();
    let (num, unit) = value.split_once(' ')?;
    let num: f64 = num.trim().parse().ok()?;
    match unit.trim().to_lowercase().as_str() {
        "gb" => Some((num * 1024.0) as u64),
        "mb" => Some(num as u64),
        _ => None,
    }
}

/// Environment variables to expose the detected GPU to the workload.
///
/// These are additive hints; user-provided environment values always win.
pub fn gpu_environment(gpu: &MacGpuInfo) -> Vec<(String, String)> {
    let mut vars = vec![
        ("EXO_GPU".to_string(), "1".to_string()),
        (
            "EXO_GPU_VENDOR".to_string(),
            gpu.vendor.as_str().to_string(),
        ),
        ("EXO_GPU_NAME".to_string(), gpu.name.clone()),
    ];

    if gpu.metal_supported {
        vars.push(("EXO_GPU_METAL".to_string(), "1".to_string()));
    }
    if let Some(cores) = gpu.cores {
        vars.push(("EXO_GPU_CORES".to_string(), cores.to_string()));
    }
    if let Some(vram) = gpu.vram_mb {
        vars.push(("EXO_GPU_VRAM_MB".to_string(), vram.to_string()));
    }

    // Framework-friendly hints for Apple GPUs (Metal Performance Shaders).
    if gpu.vendor == MacGpuVendor::Apple && gpu.metal_supported {
        vars.push(("PYTORCH_ENABLE_MPS_FALLBACK".to_string(), "1".to_string()));
    }

    vars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_apple_silicon() {
        let json = serde_json::json!({
            "SPDisplaysDataType": [{
                "_name": "Apple M1 Pro",
                "spdisplays_metal": "spdisplays_supported",
                "spdisplays_vendor": "sppci_vendor_Apple",
                "sppci_bus": "spdisplays_builtin",
                "sppci_cores": "14",
                "sppci_device_type": "spdisplays_gpu",
                "sppci_model": "Apple M1 Pro"
            }]
        });

        let gpus = parse_displays(&json);
        assert_eq!(gpus.len(), 1);
        let gpu = &gpus[0];
        assert_eq!(gpu.name, "Apple M1 Pro");
        assert_eq!(gpu.vendor, MacGpuVendor::Apple);
        assert!(gpu.metal_supported);
        assert_eq!(gpu.cores, Some(14));
        assert!(gpu.builtin);
    }

    #[test]
    fn parses_discrete_amd_vram() {
        let json = serde_json::json!({
            "SPDisplaysDataType": [{
                "sppci_model": "AMD Radeon Pro 5500M",
                "spdisplays_vendor": "sppci_vendor_amd",
                "spdisplays_metal": "spdisplays_supported",
                "spdisplays_vram": "8 GB"
            }]
        });

        let gpus = parse_displays(&json);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].vendor, MacGpuVendor::Amd);
        assert_eq!(gpus[0].vram_mb, Some(8192));
    }

    #[test]
    fn gpu_env_includes_vendor() {
        let gpu = MacGpuInfo {
            name: "Apple M1 Pro".to_string(),
            vendor: MacGpuVendor::Apple,
            metal_supported: true,
            cores: Some(14),
            vram_mb: None,
            builtin: true,
        };
        let env: Vec<_> = gpu_environment(&gpu);
        assert!(env.iter().any(|(k, v)| k == "EXO_GPU" && v == "1"));
        assert!(env
            .iter()
            .any(|(k, v)| k == "EXO_GPU_VENDOR" && v == "apple"));
        assert!(env.iter().any(|(k, _)| k == "PYTORCH_ENABLE_MPS_FALLBACK"));
    }

    #[test]
    fn parses_vram_units() {
        assert_eq!(parse_vram_mb("8 GB"), Some(8192));
        assert_eq!(parse_vram_mb("1536 MB"), Some(1536));
        assert_eq!(parse_vram_mb("weird"), None);
    }
}
