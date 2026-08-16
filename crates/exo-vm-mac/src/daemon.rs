//! Persistent host-side VM control daemon.
//!
//! Apple's `VZVirtualMachine` handle must stay alive in a process. A plain
//! `exo vm start` that creates a `VmManager` and exits immediately drops the
//! handle and tears down the VM. This daemon owns the `VmManager` and exposes a
//! small newline-delimited JSON protocol over a Unix socket so CLI commands and
//! the macOS Linux backend can talk to the live guest.

use crate::bridge::{GuestRequest, GuestResponse};
use crate::{paths, VmConfig, VmManager};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum VmDaemonRequest {
    Ping,
    Status,
    Stop { force: bool },
    Guest { request: GuestRequest },
    /// Publish a guest TCP port on the host loopback via the vsock tunnel.
    /// `container` ties the tunnel to a container name so it is dropped when
    /// that container is stopped or removed through the daemon.
    StartTunnel {
        host_port: u16,
        guest_port: u16,
        #[serde(default)]
        container: Option<String>,
    },
    StopTunnel { host_port: u16 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum VmDaemonResponse {
    Pong,
    Status {
        running: bool,
        guest_agent_reachable: bool,
        guest_agent_info: String,
    },
    Stopped,
    Guest {
        response: GuestResponse,
    },
    TunnelStarted {
        host_port: u16,
    },
    TunnelStopped {
        host_port: u16,
    },
    Error {
        message: String,
    },
}

/// Client for the VM control daemon.
#[derive(Debug, Clone)]
pub struct VmDaemonClient {
    socket_path: std::path::PathBuf,
}

impl VmDaemonClient {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            socket_path: paths::control_socket_path()?,
        })
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    pub fn is_running(&self) -> bool {
        self.request(VmDaemonRequest::Ping)
            .map(|resp| matches!(resp, VmDaemonResponse::Pong))
            .unwrap_or(false)
    }

    pub fn request(&self, req: VmDaemonRequest) -> anyhow::Result<VmDaemonResponse> {
        let mut stream = UnixStream::connect(&self.socket_path).map_err(|e| {
            anyhow::anyhow!("connect VM daemon socket {:?}: {}", self.socket_path, e)
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let line = serde_json::to_string(&req)?;
        writeln!(stream, "{}", line)?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader.read_line(&mut response)?;
        if response.trim().is_empty() {
            anyhow::bail!("empty response from VM daemon");
        }
        Ok(serde_json::from_str(response.trim())?)
    }

    pub fn guest_request(&self, req: GuestRequest) -> anyhow::Result<GuestResponse> {
        match self.request(VmDaemonRequest::Guest { request: req })? {
            VmDaemonResponse::Guest { response } => Ok(response),
            VmDaemonResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected VM daemon response: {:?}", other),
        }
    }

    pub fn status(&self) -> anyhow::Result<VmDaemonResponse> {
        self.request(VmDaemonRequest::Status)
    }

    pub fn stop(&self, force: bool) -> anyhow::Result<VmDaemonResponse> {
        self.request(VmDaemonRequest::Stop { force })
    }

    /// Ask the daemon to publish `guest_port` on host loopback `host_port`.
    pub fn start_tunnel(
        &self,
        host_port: u16,
        guest_port: u16,
        container: Option<String>,
    ) -> anyhow::Result<()> {
        match self.request(VmDaemonRequest::StartTunnel {
            host_port,
            guest_port,
            container,
        })? {
            VmDaemonResponse::TunnelStarted { .. } => Ok(()),
            VmDaemonResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected VM daemon response: {:?}", other),
        }
    }

    pub fn stop_tunnel(&self, host_port: u16) -> anyhow::Result<()> {
        match self.request(VmDaemonRequest::StopTunnel { host_port })? {
            VmDaemonResponse::TunnelStopped { .. } => Ok(()),
            VmDaemonResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected VM daemon response: {:?}", other),
        }
    }
}

/// Spawn `exo vm serve` as a detached background process with output appended
/// to the daemon log. Returns immediately; callers should poll
/// `VmDaemonClient::is_running` to wait for readiness.
pub fn spawn_detached() -> anyhow::Result<u32> {
    use std::fs::OpenOptions;
    use std::process::{Command, Stdio};

    let exe = exo_cli_path()?;
    let log_path = paths::daemon_log_path()?;
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let child = Command::new(exe)
        .args(["vm", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let pid = child.id();
    // Detach from the child. The daemon keeps the VM handle alive.
    std::mem::forget(child);
    Ok(pid)
}

/// Locate an `exo` CLI binary that can run `vm serve`: the current executable
/// when it is the CLI, `$EXO_CLI_PATH` when set, otherwise `exo` from PATH.
fn exo_cli_path() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    if exe.file_name().map(|n| n == "exo").unwrap_or(false) {
        return Ok(exe);
    }
    if let Ok(path) = std::env::var("EXO_CLI_PATH") {
        return Ok(std::path::PathBuf::from(path));
    }
    Ok(std::path::PathBuf::from("exo"))
}

/// Run the control daemon in the current process. This call does not return
/// until the VM is stopped or the listener fails.
pub fn serve_foreground(config: VmConfig) -> anyhow::Result<()> {
    let socket_path = paths::control_socket_path()?;
    if socket_path.exists() {
        // Refuse to replace a live daemon: a second listener would orphan the
        // first daemon's VM handle and double-boot the microVM.
        if UnixStream::connect(&socket_path).is_ok() {
            anyhow::bail!(
                "another exo VM daemon is already running on {}",
                socket_path.display()
            );
        }
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("VM daemon listening on {}", socket_path.display());

    let mut manager = VmManager::new(config)?;
    manager.start(false)?;
    tracing::info!("VM daemon started VM");

    let mut should_stop = false;
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                match handle_client(stream, &mut manager) {
                    Ok(true) => should_stop = true,
                    Ok(false) => {}
                    Err(e) => {
                        // A single malformed or half-open client connection must
                        // never take down the live VM handle. Log and keep serving.
                        tracing::warn!("VM daemon client error: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("VM daemon accept failed: {}", e);
            }
        }
        if should_stop {
            break;
        }
    }

    if manager.running() {
        let _ = manager.stop(false);
    }
    let _ = std::fs::remove_file(&socket_path);
    tracing::info!("VM daemon stopped");
    Ok(())
}

/// Minimum uptime before the daemon forwards requests to the guest agent.
/// Writing to the serial RPC port while the guest is still booting can wedge
/// the Virtualization.framework serial pump for the lifetime of that boot, so
/// all clients are held off until the guest agent has had time to come up.
/// First boots (ext4 format) take ~7s; steady-state boots ~3s.
const GUEST_BOOT_GRACE: Duration = Duration::from_secs(15);

fn guest_booting(manager: &VmManager) -> bool {
    manager.running()
        && manager
            .boot_elapsed()
            .map(|elapsed| elapsed < GUEST_BOOT_GRACE)
            .unwrap_or(false)
}

fn handle_client(stream: UnixStream, manager: &mut VmManager) -> anyhow::Result<bool> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let response = match serde_json::from_str::<VmDaemonRequest>(line.trim()) {
        Ok(VmDaemonRequest::Ping) => VmDaemonResponse::Pong,
        Ok(VmDaemonRequest::Status) => status_response(manager),
        Ok(VmDaemonRequest::Stop { force }) => {
            let stop_result = manager.stop(force);
            match stop_result {
                Ok(()) => VmDaemonResponse::Stopped,
                Err(e) => VmDaemonResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        Ok(VmDaemonRequest::Guest { request }) => {
            if guest_booting(manager) {
                VmDaemonResponse::Error {
                    message: "guest agent is still booting; retry shortly".to_string(),
                }
            } else {
                // StopContainer/RemoveContainer retire the container's port
                // tunnels alongside it (they were started by run via
                // StartTunnel with the container name attached).
                let tunnel_owner = match &request {
                    GuestRequest::StopContainer { id, .. }
                    | GuestRequest::RemoveContainer { id, .. } => Some(id.clone()),
                    _ => None,
                };
                match manager.guest_request(request) {
                    Ok(response) => {
                        if let Some(owner) = tunnel_owner {
                            if !matches!(response, GuestResponse::Error { .. }) {
                                let dropped = manager.stop_tunnels_for_container(&owner);
                                if !dropped.is_empty() {
                                    tracing::info!(
                                        "tunnel: dropped {:?} with container {}",
                                        dropped,
                                        owner
                                    );
                                }
                            }
                        }
                        VmDaemonResponse::Guest { response }
                    }
                    Err(e) => VmDaemonResponse::Error {
                        message: e.to_string(),
                    },
                }
            }
        }
        Ok(VmDaemonRequest::StartTunnel {
            host_port,
            guest_port,
            container,
        }) => {
            // No boot-grace gate here: the tunnel only binds the host listener
            // now; the vsock connection is opened per accepted connection.
            match manager.start_tunnel(host_port, guest_port, container) {
                Ok(()) => VmDaemonResponse::TunnelStarted { host_port },
                Err(e) => VmDaemonResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        Ok(VmDaemonRequest::StopTunnel { host_port }) => {
            if manager.stop_tunnel(host_port) {
                VmDaemonResponse::TunnelStopped { host_port }
            } else {
                VmDaemonResponse::Error {
                    message: format!("no tunnel bound to host port {}", host_port),
                }
            }
        }
        Err(e) => VmDaemonResponse::Error {
            message: format!("invalid VM daemon request: {}", e),
        },
    };

    let stop_after_response = matches!(response, VmDaemonResponse::Stopped);
    let mut writer = stream;
    writeln!(writer, "{}", serde_json::to_string(&response)?)?;
    writer.flush()?;
    Ok(stop_after_response)
}

fn status_response(manager: &VmManager) -> VmDaemonResponse {
    let running = manager.running();
    let mut guest_agent_reachable = false;
    let mut guest_agent_info = String::new();
    if running {
        if guest_booting(manager) {
            guest_agent_info = "guest agent is still booting".to_string();
        } else {
            match manager.guest_request(GuestRequest::Ping) {
                Ok(GuestResponse::Pong) => {
                    guest_agent_reachable = true;
                    guest_agent_info = "agent responded".to_string();
                }
                Ok(other) => {
                    guest_agent_info = format!("unexpected response: {:?}", other);
                }
                Err(e) => {
                    guest_agent_info = format!("agent unreachable: {}", e);
                }
            }
        }
    }
    VmDaemonResponse::Status {
        running,
        guest_agent_reachable,
        guest_agent_info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_protocol_roundtrips() {
        let req = VmDaemonRequest::Guest {
            request: GuestRequest::ListContainers { all: true },
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: VmDaemonRequest = serde_json::from_str(&json).unwrap();
        match decoded {
            VmDaemonRequest::Guest {
                request: GuestRequest::ListContainers { all },
            } => assert!(all),
            _ => panic!("unexpected request"),
        }

        let resp = VmDaemonResponse::Error {
            message: "not ready".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: VmDaemonResponse = serde_json::from_str(&json).unwrap();
        match decoded {
            VmDaemonResponse::Error { message } => assert_eq!(message, "not ready"),
            _ => panic!("unexpected response"),
        }
    }
}
