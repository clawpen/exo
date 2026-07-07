//! # exoClaw - Secure Local Agent Harness

pub mod agent;
pub mod gateway;
pub mod llm;
pub mod orchestrator;
pub mod protocol;
pub mod run_store;
pub mod runner;
pub mod tools;

pub use agent::{Agent, AgentResult};
pub use gateway::{Gateway, GatewayConfig};
pub use llm::{
    into_provider, LlmConfig, LlmProvider, LlmRequest, LlmResponse, OpenAiCompatibleProvider,
};
pub use orchestrator::{
    default_agent_roles, status_counts, AgentReport, AgentRole, AgentTask, OrchestrationState,
    OrchestrationStatus, Orchestrator, OrchestratorDecision, PrimeDirective, TaskStatus,
};
pub use protocol::{AgentConfig, AgentMessage, HostMessage, ToolDefinition, ToolPermission};
pub use run_store::{new_run_id, MailboxEvent, RunEvent, RunRecord, RunStore};
pub use runner::{
    run_to_completion, run_to_completion_with_observer, AgentExecutor, AgentPrompt,
    BuiltinExecutor, CommandAgentExecutor, ExoAgentExecutor, NoopRunObserver, RunObserver,
    RunOutcome,
};
pub use tools::{DateTimeTool, Tool, ToolRegistry};

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
