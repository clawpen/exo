//! Execute container commands via WSL2.

use crate::WslConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use tokio::process::Command as AsyncCommand;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Result from running a command in WSL.
#[derive(Debug, Clone)]
pub struct WslResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Command executor for WSL2.
pub struct WslCommand {
    config: WslConfig,
}

impl WslCommand {
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// Execute a command synchronously in WSL2.
    pub fn exec(&self, command: &str) -> Result<WslResult> {
        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.config.distro_name,
                "--user",
                "root",
                "--command",
                command,
            ])
            .output()?;

        Ok(WslResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    /// Execute a command and get stdout only.
    pub fn exec_stdout(&self, command: &str) -> Result<String> {
        let result = self.exec(command)?;
        if result.exit_code == 0 {
            Ok(result.stdout)
        } else {
            Err(anyhow::anyhow!("Command failed: {}", result.stderr))
        }
    }

    /// Start a container in WSL2.
    pub fn start_container(&self, config: &ContainerSpec) -> Result<String> {
        // Serialize the config to JSON
        let config_json = serde_json::to_string(config)?;

        // Write config to a temp file in WSL
        let temp_file = format!("/tmp/exo-{}.json", uuid::Uuid::new_v4());
        self.exec(&format!("cat << 'EOF' > {}\n{}\nEOF", temp_file, config_json))?;

        // Run the container runtime
        let container_id = self.exec_stdout(&format!(
            "exo-runtime run --config {} --id-only",
            temp_file
        ))?;

        Ok(container_id.trim().to_string())
    }

    /// Stop a running container.
    pub fn stop_container(&self, container_id: &str) -> Result<()> {
        self.exec(&format!("exo-runtime stop {}", container_id))?;
        Ok(())
    }

    /// Get container status.
    pub fn container_status(&self, container_id: &str) -> Result<ContainerStatus> {
        let output = self.exec_stdout(&format!(
            "exo-runtime status {} 2>/dev/null || echo 'unknown'",
            container_id
        ))?;

        Ok(match output.trim() {
            "running" => ContainerStatus::Running,
            "stopped" => ContainerStatus::Stopped,
            "paused" => ContainerStatus::Paused,
            _ => ContainerStatus::Unknown,
        })
    }

    /// Stream container logs.
    pub async fn stream_logs(&self, container_id: &str) -> Result<LogStream> {
        let child = AsyncCommand::new("wsl")
            .args([
                "--distribution",
                &self.config.distro_name,
                "--command",
                &format!("exo-runtime logs -f {}", container_id),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(LogStream { child })
    }
}

/// Container specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub workdir: String,
    pub env: Vec<String>,
    pub mounts: Vec<MountSpec>,
    pub gpu: bool,
    pub memory_mb: Option<u64>,
    pub cpu_shares: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

/// Container status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerStatus {
    Running,
    Stopped,
    Paused,
    Unknown,
}

/// Streaming log output from WSL.
pub struct LogStream {
    child: tokio::process::Child,
}

impl LogStream {
    pub async fn next_line(&mut self) -> Result<Option<String>> {
        if let Some(stdout) = self.child.stdout.as_mut() {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            if let Some(line) = lines.next_line().await? {
                return Ok(Some(line));
            }
        }
        Ok(None)
    }

    pub async fn wait(&mut self) -> Result<i32> {
        let status = self.child.wait().await?;
        Ok(status.code().unwrap_or(-1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_wsl_command() {
        let cmd = WslCommand::new(WslConfig::default());
        // Just verify basic echo works
        let result = cmd.exec("echo 'hello from wsl'");
        assert!(result.is_ok());
    }
}
