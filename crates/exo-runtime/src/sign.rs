//! Container image signing and verification via cosign.
//!
//! Supports key-based signing/verification (`COSIGN_PRIVATE_KEY` / `--key`)
//! and keyless verification through the standard cosign binary.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Verify an image signature before pull.
///
/// If `key_path` is `None`, cosign uses keyless (OIDC/Sigstore) verification.
/// Set `EXO_COSIGN_KEY` to a key file path for key-based verification.
pub fn verify_image(image_ref: &str, key_path: Option<&Path>) -> Result<()> {
    if !is_command_available("cosign") {
        anyhow::bail!("cosign not found in PATH; cannot verify signatures");
    }

    let mut cmd = Command::new("cosign");
    cmd.arg("verify").arg(image_ref);

    if let Some(key) = key_path {
        cmd.arg("--key").arg(key);
    }

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "Failed to run cosign verify")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cosign verify failed: {}", stderr);
    }

    Ok(())
}

/// Sign an image after push.
///
/// Uses `COSIGN_PRIVATE_KEY` env var or a key file at `key_path`. If neither
/// is provided, falls back to keyless signing (requires OIDC).
pub fn sign_image(image_ref: &str, key_path: Option<&Path>) -> Result<()> {
    if !is_command_available("cosign") {
        anyhow::bail!("cosign not found in PATH; cannot sign images");
    }

    let mut cmd = Command::new("cosign");
    cmd.arg("sign").arg(image_ref);

    if let Some(key) = key_path {
        cmd.arg("--key").arg(key);
    }

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| "Failed to run cosign sign")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cosign sign failed: {}", stderr);
    }

    Ok(())
}

/// Resolve the cosign key path from env or argument.
pub fn resolve_key_path(arg: Option<&str>) -> Option<std::path::PathBuf> {
    arg.map(|s| std::path::PathBuf::from(s)).or_else(|| {
        std::env::var("EXO_COSIGN_KEY")
            .ok()
            .map(|s| std::path::PathBuf::from(s))
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
