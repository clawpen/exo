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
}

/// Run the control daemon in the current process. This call does not return
/// until the VM is stopped or the listener fails.
pub fn serve_foreground(config: VmConfig) -> anyhow::Result<()> {
    let socket_path = paths::control_socket_path()?;
    if socket_path.exists() {
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
                if handle_client(stream, &mut manager)? {
                    should_stop = true;
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

fn handle_client(stream: UnixStream, manager: &mut VmManager) -> anyhow::Result<bool> {
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
        Ok(VmDaemonRequest::Guest { request }) => match manager.guest_request(request) {
            Ok(response) => VmDaemonResponse::Guest { response },
            Err(e) => VmDaemonResponse::Error {
                message: e.to_string(),
            },
        },
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
