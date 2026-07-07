//! Execute container commands via WSL2.

use crate::WslConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as AsyncCommand;

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
        tracing::debug!("WSL executing: {}", command);

        // Use -- shell syntax to properly execute shell commands
        // The -- signals end of WSL options, everything after goes to the shell
        let output = Command::new("wsl")
            .args([
                "-d",
                &self.config.distro_name,
                "-u",
                "root",
                "--",
                "sh",
                "-c",
                command,
            ])
            .output()?;

        tracing::debug!(
            "WSL result: exit_code={:?}, stdout='{}', stderr='{}'",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );

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
    /// Runs exo-runtime in background with proper process group handling.
    pub fn start_container(&self, spec: &ContainerSpec) -> Result<String> {
        // Build the command line args from ContainerSpec
        let mut args: Vec<String> = vec![
            "run".to_string(),
            "--name".to_string(),
            spec.name.clone(),
            "--detach".to_string(),
        ];

        // Add workdir if specified
        if !spec.workdir.is_empty() && spec.workdir != "/" {
            args.push("--workdir".to_string());
            args.push(spec.workdir.clone());
        }

        // Add environment variables
        for env_var in &spec.env {
            args.push("--env".to_string());
            args.push(env_var.clone());
        }

        // Add mounts
        for mount in &spec.mounts {
            let mount_spec = if mount.readonly {
                format!("{}:{}:ro", mount.source, mount.target)
            } else {
                format!("{}:{}", mount.source, mount.target)
            };
            args.push("--volume".to_string());
            args.push(mount_spec);
        }

        // Add image
        args.push(spec.image.clone());

        // Add command
        args.extend(spec.command.iter().cloned());

        // Run exo-runtime with setsid to create new session (daemonize)
        // Output to temp file for parsing
        let log_file = format!("/tmp/exo-start-{}.log", spec.name);
        let cmd = format!(
            "setsid exo-runtime {} > {} 2>&1 < /dev/null & PID=$!; echo $PID > /tmp/exo-pid-{}.txt; sleep 3; cat {}",
            args.join(" "),
            log_file,
            spec.name,
            log_file
        );
        tracing::debug!("Starting container with: {}", cmd);

        let output = self.exec(&cmd)?;

        // Don't fail on exit code - setsid/background causes non-zero exit
        // Parse container ID from output
        let stdout = output.stdout.trim();
        tracing::debug!("start_container output:\n{}", stdout);

        // Try to extract the container ID (UUID format)
        if let Some(uuid_line) = stdout.lines().find(|line| {
            line.contains("Container running in background:")
                || line.contains("Starting container:")
        }) {
            // Extract UUID from the line
            if let Some(uuid) = uuid_line
                .split_whitespace()
                .find(|s| s.len() == 36 && s.matches('-').count() == 4)
            {
                return Ok(uuid.to_string());
            }
        }

        // Fallback: extract any UUID from the output
        for line in stdout.lines() {
            for word in line.split_whitespace() {
                if word.len() == 36 && word.matches('-').count() == 4 {
                    return Ok(word.to_string());
                }
            }
        }

        // Final fallback: use the name as ID
        tracing::warn!(
            "Could not parse container ID from output, using name: {}",
            spec.name
        );
        Ok(spec.name.clone())
    }

    /// Run a container synchronously (for non-detached mode).
    /// Returns the exit code and combined stdout/stderr output.
    pub fn run_container_sync(&self, spec: &ContainerSpec) -> Result<(i32, String)> {
        // Build the command line args from ContainerSpec
        let mut args: Vec<String> =
            vec!["run".to_string(), "--name".to_string(), spec.name.clone()];

        // Add workdir if specified
        if !spec.workdir.is_empty() && spec.workdir != "/" {
            args.push("--workdir".to_string());
            args.push(spec.workdir.clone());
        }

        // Add environment variables
        for env_var in &spec.env {
            args.push("--env".to_string());
            args.push(env_var.clone());
        }

        // Add mounts
        for mount in &spec.mounts {
            let mount_spec = if mount.readonly {
                format!("{}:{}:ro", mount.source, mount.target)
            } else {
                format!("{}:{}", mount.source, mount.target)
            };
            args.push("--volume".to_string());
            args.push(mount_spec);
        }

        // Add image
        args.push(spec.image.clone());

        // Add command
        args.extend(spec.command.iter().cloned());

        // Run synchronously without detach - capture output
        let cmd = format!("exo-runtime {}", args.join(" "));
        tracing::debug!("Running container synchronously: {}", cmd);

        let result = self.exec(&cmd)?;

        Ok((result.exit_code, result.stdout))
    }

    /// Stop a running container.
    pub fn stop_container(&self, container_id: &str) -> Result<()> {
        self.exec(&format!("exo-runtime stop {}", container_id))?;
        Ok(())
    }

    /// Get container status.
    pub fn container_status(&self, container_id: &str) -> Result<ContainerStatus> {
        // Use 'list' command and grep for the container
        let output = self.exec(&format!(
            "exo-runtime list --all 2>/dev/null | grep -w '{}' || echo 'not_found'",
            container_id
        ))?;

        let stdout = output.stdout.trim();

        // Check if the container exists and its status
        if stdout.contains("not_found") || stdout.is_empty() {
            return Ok(ContainerStatus::Unknown);
        }

        // Parse status from list output
        // Format: <id> <name> <image> <status> ...
        if stdout.contains("running") {
            Ok(ContainerStatus::Running)
        } else if stdout.contains("stopped") || stdout.contains("exited") {
            Ok(ContainerStatus::Stopped)
        } else if stdout.contains("paused") {
            Ok(ContainerStatus::Paused)
        } else {
            Ok(ContainerStatus::Unknown)
        }
    }

    /// Stream container logs.
    pub async fn stream_logs(&self, container_id: &str) -> Result<LogStream> {
        let child = AsyncCommand::new("wsl")
            .args([
                "-d",
                &self.config.distro_name,
                "-u",
                "root",
                "exo-runtime",
                "logs",
                "-f",
                container_id,
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
