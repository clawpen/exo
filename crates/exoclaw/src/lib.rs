//! # exoClaw - Secure Local Agent Harness

pub mod protocol;
pub mod tools;
pub mod llm;
pub mod agent;
pub mod gateway;

pub use protocol::{ToolDefinition, ToolPermission, HostMessage, AgentMessage, AgentConfig};
pub use tools::{Tool, ToolRegistry, DateTimeTool};
pub use llm::{LlmProvider, LlmConfig, LlmRequest, LlmResponse, OpenAiCompatibleProvider, into_provider};
pub use agent::{Agent, AgentResult};
pub use gateway::{GatewayConfig, Gateway};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// exoClaw version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Security event for audit logging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub timestamp: String,
    pub agent_id: String,
    pub event_type: String,
    pub details: String,
    pub permitted: bool,
}

/// Error type for exoClaw
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}
