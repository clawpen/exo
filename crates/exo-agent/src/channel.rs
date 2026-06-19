//! Channel for agent communication (stdio/WebSocket).

//!
//! Currently supports:
//! - **stdio**: For container mode
//! - **WebSocket**: For remote connection (e.g., Tauri app)
//!
//! The agent uses this to communicate with the Claw Pen orchestrator
//! via the agent binary's stdin/stdout.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tracing::debug;

/// Message from orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, serde_json::Value>,
}

/// Message to orchestrator  
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputMessage {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    pub arguments: serde_json::Value,
}

/// Channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub mode: ChannelMode,
    pub websocket_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelMode {
    Stdio,
    WebSocket,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            mode: ChannelMode::Stdio,
            websocket_url: None,
        }
    }
}

/// Stdio channel implementation
pub struct StdioChannel {
    stdin: BufReader<tokio::io::Stdin>,
    stdout: BufWriter<tokio::io::Stdout>,
}

impl StdioChannel {
    pub fn new() -> Self {
        Self {
            stdin: BufReader::new(tokio::io::stdin()),
            stdout: BufWriter::new(tokio::io::stdout()),
        }
    }
    
    /// Receive a message
    pub async fn recv(&mut self) -> Result<Option<InputMessage>> {
        let mut line = String::new();
        self.stdin.read_line(&mut line).await?;
        
        if line.is_empty() {
            return Ok(None);
        }
        
        // Parse JSON
        let msg: InputMessage = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse message: {}", line))?;
        
        debug!("Received message: {:?}", msg);
        Ok(Some(msg))
    }
    
    /// Send a message
    pub async fn send(&mut self, msg: &OutputMessage) -> Result<()> {
        let json = serde_json::to_string(msg)? + "\n";
        self.stdout.write_all(json.as_bytes()).await?;
        self.stdout.flush().await?;
        Ok(())
    }
}

impl Default for StdioChannel {
    fn default() -> Self {
        Self::new()
    }
}
