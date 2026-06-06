//! Daemon client for communicating with the persistent exo daemon in WSL2.

use crate::WslConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::IntoRawSocket;
use std::path::Path;
use std::time::Duration;

const DEFAULT_SOCKET_PATH: &str = "/tmp/exo-daemon.sock";
const DEFAULT_TIMEOUT_MS: u64 = 30000;

/// Client for communicating with the exo daemon.
pub struct DaemonClient {
    socket_path: String,
    timeout: Duration,
}

impl DaemonClient {
    /// Create a new daemon client.
    pub fn new(config: &WslConfig) -> Self {
        Self {
            socket_path: DEFAULT_SOCKET_PATH.to_string(),
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
        }
    }

    /// Create a client with custom socket path.
    pub fn with_socket_path(socket_path: String) -> Self {
        Self {
            socket_path,
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
        }
    }

    /// Check if the daemon is running.
    pub fn is_running(&self) -> bool {
        // Check if the socket file exists
        self.wsl_command(&format!("test -S {}", self.socket_path))
            .map(|_output| true)  // Command succeeded = socket exists
            .unwrap_or(false)
    }

    /// Start the daemon if not running.
    pub fn ensure_running(&self) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }

        // Start the daemon in WSL2 using exo-runtime
        // The daemon runs as a background process in WSL
        let start_cmd = format!(
            "setsid exo-runtime daemon --foreground > /tmp/exo-daemon.log 2>&1 < /dev/null & echo $!"
        );

        let result = self.wsl_command(&start_cmd)?;

        // Wait for daemon to start
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(200));
            if self.is_running() {
                tracing::info!("Exo daemon started in WSL2");
                return Ok(());
            }
        }

        // Check if there was an error
        let log_output = self.wsl_command("cat /tmp/exo-daemon.log 2>/dev/null || echo 'No log yet'")?;
        tracing::warn!("Daemon log: {}", log_output);

        Err(anyhow::anyhow!("Failed to start daemon. Check WSL2 exo-runtime installation."))
    }

    /// Send a request to the daemon and get a response.
    fn send_request(&self, request: &DaemonRequest) -> Result<DaemonResponse> {
        // Use wsl+socat or wsl+nc to communicate with Unix socket
        let request_json = serde_json::to_string(request)?;

        // Build a bash command that uses socat or nc to talk to the socket
        let bash_cmd = format!(
            "echo '{}' | socat - UNIX-CONNECT:{} 2>/dev/null || echo '{{\"type\": \"error\", \"content\": {{\"message\": \"socat not available\"}}}}'",
            request_json.replace('\'', "'\\''"),
            self.socket_path
        );

        let output = self.wsl_command(&bash_cmd)?;

        // Try direct connection via UNC path if socat fails
        if output.contains("socat not available") || !output.starts_with('{') {
            return Ok(self.send_request_via_nc(request)?);
        }

        serde_json::from_str(&output).map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))
    }

    /// Send request using nc (netcat) as fallback.
    fn send_request_via_nc(&self, request: &DaemonRequest) -> Result<DaemonResponse> {
        let request_json = serde_json::to_string(request)?;

        // Write request to a temp file, then use nc
        let temp_file = format!("/tmp/exo-request-{}.json", uuid::Uuid::new_v4());
        let write_cmd = format!("echo '{}' > {}", request_json.replace('\'', "'\\''"), temp_file);
        self.wsl_command(&write_cmd)?;

        let nc_cmd = format!("nc -U {} < {} 2>/dev/null", self.socket_path, temp_file);
        let output = self.wsl_command(&nc_cmd)?;

        // Clean up temp file
        let _ = self.wsl_command(&format!("rm -f {}", temp_file));

        serde_json::from_str(&output).map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))
    }

    /// Run a container via the daemon.
    pub fn run_container(&self, spec: &ContainerSpec) -> Result<RunResult> {
        self.ensure_running()?;

        let request = DaemonRequest::Run {
            spec: spec.clone(),
        };

        match self.send_request(&request)? {
            DaemonResponse::Ok { message } => Ok(RunResult {
                success: true,
                message,
            }),
            DaemonResponse::Error { message } => Ok(RunResult {
                success: false,
                message,
            }),
            _ => Err(anyhow::anyhow!("Unexpected response from daemon")),
        }
    }

    /// Stop a container via the daemon.
    pub fn stop_container(&self, container_id: &str) -> Result<bool> {
        self.ensure_running()?;

        let request = DaemonRequest::Stop {
            container_id: container_id.to_string(),
        };

        match self.send_request(&request)? {
            DaemonResponse::Ok { .. } => Ok(true),
            DaemonResponse::Error { .. } => Ok(false),
            _ => Err(anyhow::anyhow!("Unexpected response")),
        }
    }

    /// List containers via the daemon.
    pub fn list_containers(&self, all: bool) -> Result<String> {
        self.ensure_running()?;

        let request = DaemonRequest::List { all };
        self.send_request(&request).and_then(|resp| match resp {
            DaemonResponse::List { containers } => Ok(containers),
            _ => Err(anyhow::anyhow!("Unexpected response")),
        })
    }

    /// Get container status via the daemon.
    pub fn container_status(&self, container_id: &str) -> Result<String> {
        self.ensure_running()?;

        let request = DaemonRequest::Status {
            container_id: container_id.to_string(),
        };

        self.send_request(&request).and_then(|resp| match resp {
            DaemonResponse::Status { container: _, status } => Ok(status),
            _ => Err(anyhow::anyhow!("Unexpected response")),
        })
    }

    /// Ping the daemon to check if it's alive.
    pub fn ping(&self) -> Result<bool> {
        if !self.is_running() {
            return Ok(false);
        }

        let request = DaemonRequest::Ping;
        match self.send_request(&request) {
            Ok(DaemonResponse::Pong) => Ok(true),
            _ => Ok(false),
        }
    }

    /// Execute a wsl command.
    fn wsl_command(&self, command: &str) -> Result<String> {
        let output = std::process::Command::new("wsl")
            .args(["--distribution", "Ubuntu", "--user", "root", "--command", command])
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(anyhow::anyhow!("WSL command failed: {}",
                String::from_utf8_lossy(&output.stderr)))
        }
    }
}

/// Container specification for daemon requests.
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

/// Mount specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

/// Result from running a container.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub success: bool,
    pub message: String,
}

/// Daemon request types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
enum DaemonRequest {
    #[serde(rename = "run")]
    Run { spec: ContainerSpec },

    #[serde(rename = "stop")]
    Stop { container_id: String },

    #[serde(rename = "list")]
    List { all: bool },

    #[serde(rename = "status")]
    Status { container_id: String },

    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "shutdown")]
    Shutdown,
}

/// Daemon response types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
enum DaemonResponse {
    #[serde(rename = "ok")]
    Ok { message: String },

    #[serde(rename = "error")]
    Error { message: String },

    #[serde(rename = "list")]
    List { containers: String },

    #[serde(rename = "status")]
    Status { container: String, status: String },

    #[serde(rename = "pong")]
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_spec_serialization() {
        let spec = ContainerSpec {
            id: "test-id".to_string(),
            name: "test-container".to_string(),
            image: "python:3.12".to_string(),
            command: vec!["python".to_string()],
            workdir: "/app".to_string(),
            env: vec!["TEST=value".to_string()],
            mounts: vec![],
            gpu: false,
            memory_mb: None,
            cpu_shares: None,
        };

        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("test-container"));
    }
}
