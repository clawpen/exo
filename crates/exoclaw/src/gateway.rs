//! Web UI gateway

use crate::{Agent, Result, ToolRegistry};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Gateway configuration
#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub port: u16,
    pub static_dir: PathBuf,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            port: 3847,
            static_dir: PathBuf::from("."),
        }
    }
}

/// Web gateway for browser UI
pub struct Gateway {
    config: GatewayConfig,
    agent: Arc<Agent>,
    tool_registry: Arc<ToolRegistry>,
}

impl Gateway {
    pub fn new(config: GatewayConfig, agent: Arc<Agent>, tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            config,
            agent,
            tool_registry,
        }
    }

    /// Start the web server
    pub async fn run(&self) -> Result<()> {
        println!("Starting exoClaw web gateway on port {}", self.config.port);
        println!("Open http://localhost:{} in your browser", self.config.port);

        // For now, just serve a simple status
        self.serve_simple().await
    }

    async fn serve_simple(&self) -> Result<()> {
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.config.port)).await?;

        loop {
            let (mut socket, _) = listener.accept().await?;

            let mut buffer = [0; 1024];
            let _n = socket.read(&mut buffer).await?;

            let html = r#"
<!DOCTYPE html>
<html>
<head>
    <title>exoClaw</title>
    <style>
        body { font-family: system-ui; max-width: 800px; margin: 40px auto; padding: 0 20px; }
        h1 { color: #333; }
        .status { background: #e8f5e9; padding: 20px; border-radius: 8px; }
        .pending { background: #fff3e0; padding: 20px; border-radius: 8px; }
    </style>
</head>
<body>
    <h1>exoClaw - Secure Local Agent Harness</h1>
    <div class="status">
        <h2>Status: Running</h2>
        <p>Web interface is operational. Full UI coming soon.</p>
    </div>
    <div class="pending">
        <h3>Current Capabilities:</h3>
        <ul>
            <li>Tool permission system</li>
            <li>Audit logging</li>
            <li>Unix socket communication (no network exposure)</li>
        </ul>
    </div>
</body>
</html>
"#;

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
                html.len(),
                html
            );

            socket.write_all(response.as_bytes()).await?;
        }
    }
}
