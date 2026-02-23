//! RPC server for communication with Windows host.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::error;

/// Run the RPC server.
#[cfg(target_os = "linux")]
pub async fn run(socket_path: &str) -> Result<()> {
    use tokio::net::UnixListener;

    // Clean up old socket if it exists
    if std::path::Path::new(socket_path).exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    tracing::info!("OpenClaw runtime server listening on: {}", socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream).await {
                        error!("Connection error: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Accept error: {}", e);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn run(_socket_path: &str) -> Result<()> {
    Err(anyhow::anyhow!("RPC server only supported on Linux"))
}

#[cfg(target_os = "linux")]
async fn handle_connection(mut stream: tokio::net::UnixStream) -> Result<()> {
    let (reader, mut writer) = stream.split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        use tokio::io::AsyncBufReadExt;
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }

        let request: Request = serde_json::from_str(&line.trim())?;
        let response = handle_request(request)?;

        writer.write_all(response.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
async fn handle_connection(_stream: std::net::TcpStream) -> Result<()> {
    Ok(())
}

fn handle_request(request: Request) -> Result<String> {
    let response = match request {
        Request::Ping => Response::Pong,
        Request::Run { config } => {
            // Run container
            if let Ok(_container) = crate::container::Container::from_spec(config) {
                // In production, we'd actually start it here
                Response::Started { id: "test-id".to_string() }
            } else {
                Response::Error { message: "Failed to create container".to_string() }
            }
        }
        Request::Stop { container_id } => {
            Response::Stopped { container_id }
        }
        Request::Status { container_id } => {
            let status = crate::state::get_status(&container_id)?
                .unwrap_or("unknown".to_string());
            Response::Status { container_id, status }
        }
    };

    Ok(serde_json::to_string(&response)?)
}

/// RPC request types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum Request {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "run")]
    Run { config: serde_json::Value },
    #[serde(rename = "stop")]
    Stop { container_id: String },
    #[serde(rename = "status")]
    Status { container_id: String },
}

/// RPC response types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum Response {
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "started")]
    Started { id: String },
    #[serde(rename = "stopped")]
    Stopped { container_id: String },
    #[serde(rename = "status")]
    Status { container_id: String, status: String },
    #[serde(rename = "error")]
    Error { message: String },
}
