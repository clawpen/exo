//! WSL deployment - installs and manages the Linux runtime in WSL2

use crate::{WslCommand, WslConfig};
use anyhow::Result;
use std::path::{Path, PathBuf};
use base64::prelude::*;

const RUNTIME_BINARY_NAME: &str = "exo-runtime";
const RUNTIME_INSTALL_PATH: &str = "/usr/local/bin";
const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Manages deployment of the Linux runtime to WSL2
pub struct WslDeployer {
    config: WslConfig,
}

impl WslDeployer {
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// Check if the Linux runtime is installed in WSL
    pub fn is_runtime_installed(&self) -> Result<bool> {
        let cmd = WslCommand::new(self.config.clone());
        let result = cmd.exec(&format!("which {}", RUNTIME_BINARY_NAME))?;

        Ok(result.exit_code == 0)
    }

    /// Get the version of installed runtime
    pub fn installed_version(&self) -> Result<Option<String>> {
        let cmd = WslCommand::new(self.config.clone());
        let result = cmd.exec(&format!("{} --version", RUNTIME_BINARY_NAME))?;

        if result.exit_code == 0 {
            Ok(Some(result.stdout.trim().to_string()))
        } else {
            Ok(None)
        }
    }

    /// Deploy the Linux runtime to WSL
    ///
    /// This copies the runtime binary from Windows to WSL and installs it
    pub fn deploy_runtime(&self) -> Result<()> {
        tracing::info!("Deploying Exo runtime to WSL...");

        // Check if runtime is already installed
        if self.is_runtime_installed()? {
            let installed_version = self.installed_version()?;
            tracing::info!("Runtime already installed: {:?}", installed_version);
            return Ok(());
        }

        // For development, we look for the runtime in the project
        // In production, this would download from GitHub releases
        let project_root = self.get_project_root()?;
        let runtime_binary = Self::find_runtime_binary(&project_root)?;

        if !runtime_binary.exists() {
            anyhow::bail!(
                "Runtime binary not found at {:?}.\n\
                 Build with: cargo build --release -p exo-runtime\n\
                 Or: cargo install --path . --bin",
                runtime_binary
            );
        }

        tracing::info!("Found runtime binary at: {:?}", runtime_binary);

        // Copy runtime to WSL
        let wsl_temp = "/tmp/exo-runtime-deploy";
        self.copy_to_wsl(&runtime_binary.to_string_lossy(), wsl_temp)?;

        // Install to /usr/local/bin
        let cmd = WslCommand::new(self.config.clone());
        cmd.exec(&format!(
            "install -m 755 {} {}",
            wsl_temp,
            RUNTIME_INSTALL_PATH
        ))?;

        // Remove temp file
        cmd.exec(&format!("rm -f {}", wsl_temp))?;

        // Verify installation
        if !self.is_runtime_installed()? {
            anyhow::bail!("Failed to install runtime");
        }

        tracing::info!("Runtime deployed successfully");
        Ok(())
    }

    /// Find the runtime binary in the project
    fn get_project_root(&self) -> Result<PathBuf> {
        // Start from current executable and go up to project root
        let exe_path = std::env::current_exe()?;

        let mut path = exe_path.as_path();

        // Go up through directories until we find the project marker
        for _ in 0..5 {
            if path.join("Cargo.toml").exists() {
                return Ok(path.to_path_buf());
            }
            path = path.parent().unwrap_or(Path::new("."));
        }

        Err(anyhow::anyhow!("Could not find project root"))
    }

    /// Find the runtime binary in the project
    fn find_runtime_binary(project_root: &Path) -> Result<PathBuf> {
        // Check for release build
        let release_path = project_root.join("target/release");

        #[cfg(target_os = "linux")]
        let binary_name = "exo-runtime";
        #[cfg(not(target_os = "linux"))]
        let binary_name = "exo-runtime";

        #[cfg(target_os = "linux")]
        let alt_path = project_root.join("target/x86_64-unknown-linux-gnu/release");
        #[cfg(not(target_os = "linux"))]
        let alt_path = project_root.join("target/release");

        // Try primary path first
        let binary_path = release_path.join(binary_name);
        if binary_path.exists() {
            return Ok(binary_path);
        }

        #[cfg(target_os = "linux")]
        if alt_path.exists() {
            return Ok(alt_path.join(binary_name));
        }

        anyhow::bail!("Runtime binary not found. Run: cargo build --release -p exo-runtime")
    }

    /// Copy a file from Windows to WSL
    fn copy_to_wsl(&self, windows_path: &str, wsl_path: &str) -> Result<()> {
        let cmd = WslCommand::new(self.config.clone());

        // Read file content as base64 for safe shell transfer
        let content = std::fs::read(windows_path)?;
        let encoded = base64::prelude::BASE64_STANDARD.encode(&content);

        // Decode base64 in WSL and write to file
        cmd.exec(&format!(
            "echo '{}' | base64 -d > {} && chmod +x {}",
            encoded, wsl_path, wsl_path
        ))?;

        Ok(())
    }

    /// Ensure WSL is properly initialized
    pub fn ensure_wsl_ready(&self) -> Result<()> {
        // Check WSL is installed
        let cmd = WslCommand::new(self.config.clone());
        let result = cmd.exec("--version")?;
        if result.exit_code != 0 {
            anyhow::bail!("WSL not available. Install WSL2 first.");
        }

        // Check the distro exists
        let result = cmd.exec(&format!("wsl -l -q {}", self.config.distro_name))?;
        if result.exit_code != 0 {
            tracing::info!("Distro '{}' not found, initializing...", self.config.distro_name);
            self.initialize_distro()?;
        }

        Ok(())
    }

    /// Initialize the WSL distro for Exo
    fn initialize_distro(&self) -> Result<()> {
        let cmd = WslCommand::new(self.config.clone());

        tracing::info!("Creating Exo WSL distro...");

        // Import or create the distro
        let result = cmd.exec(&format!(
            "wsl --import {} ${{HOME}}/.local/share/wsl/distributions/{} 2>/dev/null || \
             wsl --install -d {}",
            self.config.distro_name,
            self.config.distro_name,
            "Ubuntu-22.04"
        ))?;

        if result.exit_code != 0 {
            anyhow::bail!("Failed to initialize WSL distro");
        }

        tracing::info!("WSL distro ready");
        Ok(())
    }

    /// Cleanup stale container state in WSL
    pub fn cleanup_stale_containers(&self) -> Result<()> {
        let cmd = WslCommand::new(self.config.clone());

        // Remove state directory for containers that no longer exist
        cmd.exec(&format!(
            "find /var/lib/exo/containers -name '.*' -type d -empty -exec rmdir {} \\;",
            self.config.distro_name
        ))?;

        Ok(())
    }

    /// Get the state directory for containers in WSL
    pub fn state_dir(&self) -> String {
        format!("/var/lib/exo/containers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_wsl_deployer() {
        let deployer = WslDeployer::new(WslConfig::default());
        // Just verify it can be created
        assert_eq!(deployer.config.distro_name, "exo");
    }
}