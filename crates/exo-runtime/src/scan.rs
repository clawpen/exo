//! Vulnerability scanning hook for pulled/built images.
//!
//! Shells out to `grype` or `trivy` on the composed rootfs and parses the
//! JSON report. Failures are logged but do not block the pull/build unless
//! `EXO_SCAN_FATAL=1` is set.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::{Command, Stdio};

/// A single vulnerability finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vulnerability {
    pub id: String,
    pub severity: String,
    pub package: String,
    pub version: String,
    pub fix_version: Option<String>,
    pub description: Option<String>,
}

/// Scan result for an image.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VulnerabilityReport {
    pub scanner: String,
    pub image: String,
    pub scanned_at: String,
    pub vulnerabilities: Vec<Vulnerability>,
}

impl VulnerabilityReport {
    pub fn critical_count(&self) -> usize {
        self.count_by_severity("critical")
    }

    pub fn high_count(&self) -> usize {
        self.count_by_severity("high")
    }

    pub fn medium_count(&self) -> usize {
        self.count_by_severity("medium")
    }

    pub fn low_count(&self) -> usize {
        self.count_by_severity("low")
    }

    fn count_by_severity(&self, severity: &str) -> usize {
        self.vulnerabilities
            .iter()
            .filter(|v| v.severity.eq_ignore_ascii_case(severity))
            .count()
    }
}

/// Scan a composed image rootfs for vulnerabilities.
///
/// Prefers `grype`; falls back to `trivy`. Returns an empty report if no
/// scanner is installed.
pub fn scan_image_rootfs(image_name: &str, rootfs: &Path) -> Result<VulnerabilityReport> {
    if is_command_available("grype") {
        scan_with_grype(image_name, rootfs)
    } else if is_command_available("trivy") {
        scan_with_trivy(image_name, rootfs)
    } else {
        tracing::warn!(
            "No vulnerability scanner found (grype or trivy). Skipping scan for {}.",
            image_name
        );
        Ok(VulnerabilityReport {
            scanner: "none".to_string(),
            image: image_name.to_string(),
            scanned_at: now_iso(),
            vulnerabilities: vec![],
        })
    }
}

fn scan_with_grype(image_name: &str, rootfs: &Path) -> Result<VulnerabilityReport> {
    let output = Command::new("grype")
        .arg(rootfs)
        .args(["-o", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "Failed to run grype")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("grype failed: {}", stderr);
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| "Failed to parse grype JSON output")?;

    let matches = json.get("matches").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let vulnerabilities = matches
        .into_iter()
        .filter_map(|m| parse_grype_match(m))
        .collect();

    Ok(VulnerabilityReport {
        scanner: "grype".to_string(),
        image: image_name.to_string(),
        scanned_at: now_iso(),
        vulnerabilities,
    })
}

fn parse_grype_match(m: serde_json::Value) -> Option<Vulnerability> {
    let vuln = m.get("vulnerability")?;
    let artifact = m.get("artifact")?;
    Some(Vulnerability {
        id: vuln.get("id")?.as_str()?.to_string(),
        severity: vuln
            .get("severity")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        package: artifact
            .get("name")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        version: artifact
            .get("version")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        fix_version: vuln
            .get("fix")
            .and_then(|f| f.get("versions"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(String::from),
        description: vuln
            .get("description")
            .and_then(|s| s.as_str())
            .map(String::from),
    })
}

fn scan_with_trivy(image_name: &str, rootfs: &Path) -> Result<VulnerabilityReport> {
    let output = Command::new("trivy")
        .arg("filesystem")
        .arg(rootfs)
        .args(["--format", "json", "--scanners", "vuln"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "Failed to run trivy")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("trivy failed: {}", stderr);
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| "Failed to parse trivy JSON output")?;

    let results = json.get("Results").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    let vulnerabilities = results
        .into_iter()
        .flat_map(|r| {
            r.get("Vulnerabilities")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|v| parse_trivy_vulnerability(v))
        .collect();

    Ok(VulnerabilityReport {
        scanner: "trivy".to_string(),
        image: image_name.to_string(),
        scanned_at: now_iso(),
        vulnerabilities,
    })
}

fn parse_trivy_vulnerability(v: serde_json::Value) -> Option<Vulnerability> {
    Some(Vulnerability {
        id: v.get("VulnerabilityID")?.as_str()?.to_string(),
        severity: v
            .get("Severity")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        package: v
            .get("PkgName")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        version: v
            .get("InstalledVersion")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown")
            .to_string(),
        fix_version: v
            .get("FixedVersion")
            .and_then(|s| s.as_str())
            .map(String::from),
        description: v
            .get("Description")
            .and_then(|s| s.as_str())
            .map(String::from),
    })
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

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_counts() {
        let report = VulnerabilityReport {
            scanner: "test".to_string(),
            image: "test".to_string(),
            scanned_at: now_iso(),
            vulnerabilities: vec![
                Vulnerability {
                    id: "CVE-1".to_string(),
                    severity: "Critical".to_string(),
                    package: "pkg".to_string(),
                    version: "1.0".to_string(),
                    fix_version: None,
                    description: None,
                },
                Vulnerability {
                    id: "CVE-2".to_string(),
                    severity: "High".to_string(),
                    package: "pkg".to_string(),
                    version: "1.0".to_string(),
                    fix_version: None,
                    description: None,
                },
                Vulnerability {
                    id: "CVE-3".to_string(),
                    severity: "High".to_string(),
                    package: "pkg".to_string(),
                    version: "1.0".to_string(),
                    fix_version: None,
                    description: None,
                },
            ],
        };
        assert_eq!(report.critical_count(), 1);
        assert_eq!(report.high_count(), 2);
        assert_eq!(report.medium_count(), 0);
    }
}
