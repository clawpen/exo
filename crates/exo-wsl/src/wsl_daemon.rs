//! WSL2 daemon implementation - runs as a Unix socket server in WSL2
//!
//! This daemon runs inside WSL2 and provides fast container operations
//! by keeping the exo-runtime loaded in memory.

use crate::WslConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::thread;
use std::time::Duration;

const SOCKET_PATH: &str = "/tmp/exo-daemon.sock";
const PID_FILE: &str = "/tmp/exo-daemon.pid";

/// Check if the daemon is running in WSL2
#[cfg(target_os = "linux")]
pub fn is_daemon_running() -> bool {
    Path::new(SOCKET_PATH).exists()
}

/// Start the daemon in WSL2
#[cfg(target_os = "linux")]
pub fn start_daemon() -> Result<()> {
    use std::os::unix::net::UnixListener;
    use std::os::unix::fs::PermissionsExt;
    use std::io::{Read, Write};
    use std::process::Command;

    // Clean up old socket if it exists
    let _ = std::fs::remove_file(SOCKET_PATH);

    // Write PID file
    let _ = std::fs::write(PID_FILE, format!("{}\n", std::process::id()));

    // Bind to socket
    let listener = UnixListener::bind(SOCKET_PATH)?;

    println!("Exo WSL daemon listening on: {}", SOCKET_PATH);

    // Set socket permissions
    let _ = std::fs::set_permissions(SOCKET_PATH, PermissionsExt::from_mode(0o777));

    // Accept connections
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(e) = handle_connection(stream) {
                    eprintln!("Error handling connection: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Connection failed: {}", e);
            }
        }
    }

    Ok(())
}

/// Handle a client connection
#[cfg(target_os = "linux")]
fn handle_connection(mut stream: std::os::unix::net::UnixStream) -> Result<()> {
    // Clean up old socket if it exists
    let _ = std::fs::remove_file(SOCKET_PATH);

    // Write PID file
    let _ = std::fs::write(PID_FILE, format!("{}\n", std::process::id()));

    // Bind to socket
    let listener = UnixListener::bind(SOCKET_PATH)?;

    println!("Exo WSL daemon listening on: {}", SOCKET_PATH);

    // Set socket permissions
    let _ = std::fs::set_permissions(SOCKET_PATH, std::os::unix::fs::PermissionsExt::from_mode(0o777));

    // Accept connections
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(e) = handle_connection(stream) {
                    eprintln!("Error handling connection: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Connection failed: {}", e);
            }
        }
    }

    Ok(())
}

/// Handle a client connection
#[cfg(target_os = "linux")]
fn handle_connection(mut stream: std::os::unix::net::UnixStream) -> Result<()> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::net::Shutdown;

    // Set read timeout
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;

    // Read request
    let mut buffer = [0u8; 8192];
    let n = stream.read(&mut buffer)?;
    let request_json = String::from_utf8(buffer[..n].to_vec())?;

    // Parse request
    let request: DaemonRequest = serde_json::from_str(&request_json)?;

    // Process request and send response
    let response = process_request(request)?;

    let response_json = serde_json::to_string(&response)?;
    stream.write_all(response_json.as_bytes())?;
    stream.shutdown(Shutdown::Both)?;

    Ok(())
}

/// Process a daemon request
#[cfg(target_os = "linux")]
fn process_request(request: DaemonRequest) -> Result<DaemonResponse> {
    use std::process::Command;

    match request {
        DaemonRequest::Run { spec } => {
            // Build exo-runtime command
            let mut args = vec!["run".to_string(), "--name".to_string(), spec.name.clone()];

            if !spec.workdir.is_empty() && spec.workdir != "/" {
                args.push("--workdir".to_string());
                args.push(spec.workdir);
            }

            for env in &spec.env {
                args.push("--env".to_string());
                args.push(env.clone());
            }

            for mount in &spec.mounts {
                let mount_str = if mount.readonly {
                    format!("{}:{}:ro", mount.source, mount.target)
                } else {
                    format!("{}:{}", mount.source, mount.target)
                };
                args.push("--volume".to_string());
                args.push(mount_str);
            }

            args.push(spec.image);
            args.extend(spec.command.clone());

            // Run with setsid for proper daemonization
            let cmd = format!(
                "setsid exo-runtime {} > /tmp/exo-daemon-out.log 2>&1 < /dev/null & echo $!",
                args.join(" ")
            );

            let output = Command::new("bash")
                .args(["-c", &cmd])
                .output()?;

            if output.status.success() {
                Ok(DaemonResponse::Ok {
                    message: format!("Container {} started", spec.name),
                })
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Ok(DaemonResponse::Error {
                    message: format!("Failed to start container: {}", stderr),
                })
            }
        }

        DaemonRequest::Stop { container_id } => {
            let output = Command::new("exo-runtime")
                .args(["stop", &container_id])
                .output()?;

            if output.status.success() {
                Ok(DaemonResponse::Ok {
                    message: format!("Container {} stopped", container_id),
                })
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Ok(DaemonResponse::Error {
                    message: format!("Failed to stop container: {}", stderr),
                })
            }
        }

        DaemonRequest::List { all } => {
            let arg = if all { "--all" } else { "" };
            let output = Command::new("exo-runtime")
                .args(["list", arg])
                .output()?;

            if output.status.success() {
                Ok(DaemonResponse::List {
                    containers: String::from_utf8_lossy(&output.stdout).to_string(),
                })
            } else {
                Ok(DaemonResponse::List {
                    containers: String::new(),
                })
            }
        }

        DaemonRequest::Status { container_id } => {
            let output = Command::new("exo-runtime")
                .args(["list", "--all"])
                .output()?;

            let stdout = String::from_utf8_lossy(&output.stdout);

            // Parse status from list output
            let status = if stdout.contains(&container_id) {
                if stdout.lines()
                    .any(|l| l.contains(&container_id) && l.contains("running"))
                {
                    "running".to_string()
                } else {
                    "stopped".to_string()
                }
            } else {
                "not found".to_string()
            };

            Ok(DaemonResponse::Status {
                container: container_id,
                status,
            })
        }

        DaemonRequest::Ping => Ok(DaemonResponse::Pong),

        DaemonRequest::Shutdown => {
            // Remove socket file and exit
            let _ = std::fs::remove_file(SOCKET_PATH);
            let _ = std::fs::remove_file(PID_FILE);
            std::process::exit(0);
        }
    }
}

/// Stop the daemon
#[cfg(target_os = "linux")]
pub fn stop_daemon() -> Result<()> {
    if let Ok(pid_str) = std::fs::read_to_string(PID_FILE) {
        let pid: u32 = pid_str.trim().parse()?;

        let output = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()?;

        if output.status.success() {
            println!("Daemon stopped (PID: {})", pid);

            // Wait a moment and clean up socket
            std::thread::sleep(Duration::from_millis(100));
            let _ = std::fs::remove_file(SOCKET_PATH);
            let _ = std::fs::remove_file(PID_FILE);

            return Ok(());
        }
    }

    // Try to remove socket if it exists
    if Path::new(SOCKET_PATH).exists() {
        let _ = std::fs::remove_file(SOCKET_PATH);
    }

    println!("Daemon socket cleaned up");
    Ok(())
}

/// Daemon request types
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

/// Daemon response types
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

/// Container specification
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

// Stub for non-Unix platforms
#[cfg(not(target_os = "linux"))]
pub fn is_daemon_running() -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub fn start_daemon() -> Result<()> {
    Err(anyhow::anyhow!("Daemon mode is only supported on Linux/WSL2"))
}

#[cfg(not(target_os = "linux"))]
pub fn stop_daemon() -> Result<()> {
    Err(anyhow::anyhow!("Daemon mode is only supported on Linux/WSL2"))
}
