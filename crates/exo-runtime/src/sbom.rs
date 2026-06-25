//! SBOM generation for pulled/built images.
//!
//! Shells out to `syft` (preferred) or `trivy sbom` on a composed rootfs and
//! stores the SBOM as JSON next to the image store.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Supported SBOM formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbomFormat {
    SpdxJson,
    CycloneDxJson,
}

impl SbomFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            SbomFormat::SpdxJson => "spdx-json",
            SbomFormat::CycloneDxJson => "cyclonedx-json",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            SbomFormat::SpdxJson => "spdx.json",
            SbomFormat::CycloneDxJson => "cdx.json",
        }
    }
}

impl Default for SbomFormat {
    fn default() -> Self {
        SbomFormat::SpdxJson
    }
}

/// Generate an SBOM for a composed image rootfs.
///
/// Prefers `syft`; falls back to `trivy sbom`. Returns the SBOM JSON string.
pub fn generate_sbom(image_name: &str, rootfs: &Path, format: SbomFormat) -> Result<String> {
    if is_command_available("syft") {
        generate_with_syft(rootfs, format)
    } else if is_command_available("trivy") {
        generate_with_trivy(rootfs, format)
    } else {
        anyhow::bail!(
            "No SBOM generator found (syft or trivy). Skipping SBOM for {}.",
            image_name
        )
    }
}

fn generate_with_syft(rootfs: &Path, format: SbomFormat) -> Result<String> {
    let output = Command::new("syft")
        .arg(rootfs)
        .args(["-o", format.as_str()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "Failed to run syft")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("syft failed: {}", stderr);
    }

    String::from_utf8(output.stdout)
        .with_context(|| "syft output is not valid UTF-8")
}

fn generate_with_trivy(rootfs: &Path, format: SbomFormat) -> Result<String> {
    let trivy_format = match format {
        SbomFormat::SpdxJson => "spdx-json",
        SbomFormat::CycloneDxJson => "cyclonedx",
    };
    let output = Command::new("trivy")
        .arg("filesystem")
        .arg(rootfs)
        .args(["--format", trivy_format, "--scanners", "vuln"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "Failed to run trivy sbom")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("trivy sbom failed: {}", stderr);
    }

    String::from_utf8(output.stdout)
        .with_context(|| "trivy sbom output is not valid UTF-8")
}

/// Save an SBOM to the image store's `sboms/` directory.
pub fn save_sbom(
    store_root: &Path,
    image_name: &str,
    format: SbomFormat,
    sbom_json: &str,
) -> Result<PathBuf> {
    let sbom_dir = store_root.join("sboms");
    std::fs::create_dir_all(&sbom_dir)
        .with_context(|| format!("Failed to create sbom dir {:?}", sbom_dir))?;

    let filename = format!("{}.{} ", image_name.replace([':', '/'], "_"), format.extension());
    let path = sbom_dir.join(filename);
    std::fs::write(&path, sbom_json)
        .with_context(|| format!("Failed to write SBOM to {:?}", path))?;
    Ok(path)
}

fn is_command_available(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sbom_format_strings() {
        assert_eq!(SbomFormat::SpdxJson.as_str(), "spdx-json");
        assert_eq!(SbomFormat::CycloneDxJson.extension(), "cdx.json");
    }
}
