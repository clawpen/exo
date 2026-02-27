//! WSL2 distro management.

use crate::WslConfig;
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;
use tracing::{info, debug, warn};

/// Represents a WSL2 distro managed by OpenClaw.
pub struct WslDistro {
    config: WslConfig,
}

impl WslDistro {
    /// Create a new WSL distro manager.
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// Check if the OpenClaw distro exists.
    pub fn exists(&self) -> bool {
        match Command::new("wsl")
            .args(["--list", "--quiet"])
            .output()
        {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.lines().any(|line| line.trim() == self.config.distro_name)
            }
            Err(_) => false,
        }
    }

    /// Create a new WSL2 distro for OpenClaw.
    pub fn create(&self) -> Result<()> {
        info!("Creating OpenClaw WSL2 distro: {}", self.config.distro_name);

        // Import a minimal distro
        // For now, we'll assume the user has Ubuntu or similar installed
        // In production, we'd ship a minimal rootfs

        // Check if we need to install WSL first
        self.ensure_wsl_installed()?;

        // Create the distro using wsl --import
        let distro_path = self.expand_path(&self.config.distro_path)?;
        std::fs::create_dir_all(&distro_path)?;

        // Download or use a base image (Ubuntu minimal)
        // For now, use wsl --import with Ubuntu
        let output = Command::new("wsl")
            .args([
                "--import",
                &self.config.distro_name,
                &distro_path.to_string_lossy(),
                "https://aka.ms/wslubuntu2204", // Or ship our own rootfs
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Might already exist, that's ok
            warn!("WSL import output: {}", stderr);
        }

        Ok(())
    }

    /// Ensure the distro exists, creating if necessary.
    pub fn ensure(&self) -> Result<()> {
        if !self.exists() {
            self.create()?;
        }

        // Set WSL2 as the default version for this distro
        Command::new("wsl")
            .args(["--set-default-version", "2"])
            .status()?;

        Ok(())
    }

    /// Run a command inside the WSL distro.
    pub fn exec(&self, command: &str) -> Result<String> {
        debug!("Executing in WSL: {}", command);

        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.config.distro_name,
                "--command",
                command,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("WSL command failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Run a command inside the WSL distro and return exit code.
    pub fn exec_status(&self, command: &str) -> Result<i32> {
        debug!("Executing in WSL (status check): {}", command);

        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.config.distro_name,
                "--command",
                command,
            ])
            .output()?;

        Ok(output.status.code().unwrap_or(-1))
    }

    /// Copy a file from Windows to WSL.
    pub fn copy_in(&self, source: &str, dest: &str) -> Result<()> {
        let wsl_path = format!("\\\\wsl$\\{}\\{}", self.config.distro_name, dest);
        std::fs::copy(source, &wsl_path)?;
        Ok(())
    }

    /// Copy a file from WSL to Windows.
    pub fn copy_out(&self, source: &str, dest: &str) -> Result<()> {
        let wsl_path = format!("\\\\wsl$\\{}\\{}", self.config.distro_name, source);
        std::fs::copy(&wsl_path, dest)?;
        Ok(())
    }

    /// Shutdown the WSL distro.
    pub fn shutdown(&self) -> Result<()> {
        info!("Shutting down WSL distro: {}", self.config.distro_name);
        Command::new("wsl")
            .args(["--terminate", &self.config.distro_name])
            .status()?;
        Ok(())
    }

    /// Get the path to the distro inside WSL.
    pub fn wsl_path(&self) -> String {
        format!("/mnt/wsl/{}", self.config.distro_name)
    }

    fn ensure_wsl_installed(&self) -> Result<()> {
        // Check if WSL is installed
        let output = Command::new("wsl")
            .args(["--status"])
            .output();

        match output {
            Ok(o) if o.status.success() => Ok(()),
            _ => {
                warn!("WSL not installed. Please install WSL2.");
                Err(anyhow::anyhow!("WSL2 is not installed. Run: wsl --install"))
            }
        }
    }

    fn expand_path(&self, path: &str) -> Result<PathBuf> {
        // Expand environment variables like %LOCALAPPDATA%
        if path.contains('%') {
            if let (Some(start), Some(end)) = (path.find('%'), path.rfind('%')) {
                let var = &path[start + 1..end];
                if let Ok(value) = std::env::var(var) {
                    let expanded = format!("{}{}{}", &path[..start], value, &path[end + 1..]);
                    return Ok(PathBuf::from(expanded));
                }
            }
        }
        Ok(PathBuf::from(path))
    }
}

/// Manages multiple WSL distros for OpenClaw.
pub struct WslDistroManager {
    config: WslConfig,
}

impl WslDistroManager {
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// Get the main OpenClaw distro.
    pub fn distro(&self) -> WslDistro {
        WslDistro::new(self.config.clone())
    }

    /// Initialize the OpenClaw WSL environment.
    pub fn initialize(&self) -> Result<()> {
        info!("Initializing OpenClaw WSL environment");

        let distro = self.distro();

        // Ensure distro exists
        distro.ensure()?;

        // Install necessary packages inside WSL
        distro.exec("apt-get update -qq")?;
        distro.exec("apt-get install -y -qq uidmap fuse3 libfuse3-dev")?;

        // Deploy the OpenClaw runtime binary
        self.deploy_runtime(&distro)?;

        Ok(())
    }

    fn deploy_runtime(&self, distro: &WslDistro) -> Result<()> {
        info!("Deploying OpenClaw runtime to WSL");

        // Copy the Linux runtime binary into WSL
        // In production, this would be embedded in the Windows exe
        let runtime_path = std::env::current_exe()?
            .parent()
            .map(|p| p.join("openclaw-runtime"))
            .unwrap_or_else(|| PathBuf::from("openclaw-runtime"));

        if runtime_path.exists() {
            distro.copy_in(runtime_path.to_str().unwrap(), "/usr/local/bin/openclaw-runtime")?;
            distro.exec("chmod +x /usr/local/bin/openclaw-runtime")?;
        } else {
            // For development, we'll compile it in-place
            distro.exec("which openclaw-runtime || echo 'Runtime not yet deployed'")?;
        }

        Ok(())
    }
}

impl Default for WslDistroManager {
    fn default() -> Self {
        Self::new(WslConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_distro_exists() {
        let distro = WslDistro::new(WslConfig::default());
        // Just verify it doesn't panic
        let _exists = distro.exists();
    }
}
