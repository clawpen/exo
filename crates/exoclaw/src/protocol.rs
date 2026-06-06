//! Agent communication protocol

use serde::{Deserialize, Serialize};

/// Message from host to agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum HostMessage {
    /// Initial handshake with agent configuration
    #[serde(rename = "handshake")]
    Handshake { config: AgentConfig },

    /// Task request from user
    #[serde(rename = "task")]
    Task { task: String },

    /// Ping/heartbeat
    #[serde(rename = "ping")]
    Ping,
}

/// Message from agent to host
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum AgentMessage {
    /// Agent is ready
    #[serde(rename = "ready")]
    Ready,

    /// Agent has completed the task
    #[serde(rename = "done")]
    Done { output: String },

    /// Agent encountered an error
    #[serde(rename = "error")]
    Error { message: String },

    /// Pong response
    #[serde(rename = "pong")]
    Pong,
}

/// Configuration passed to agent on startup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_id: String,
    pub agent_name: String,
    pub tools: Vec<ToolDefinition>,
    pub user: String,
    pub workspace: String,
}

/// Tool definition exposed to agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub requires_permission: ToolPermission,
}

/// Permission level required for tool
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolPermission {
    /// No permission needed - always allowed
    #[serde(rename = "none")]
    None,

    /// Ask user once per session
    #[serde(rename = "once")]
    Once,

    /// Ask every time
    #[serde(rename = "always")]
    Always,

    /// Never allow this tool
    #[serde(rename = "never")]
    Never,
}

/// Tool call request from agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

/// Result of tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}
