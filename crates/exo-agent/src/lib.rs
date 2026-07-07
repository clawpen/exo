//! Exo Agent - Lightweight agent runtime for Exo containers
//!
//! A minimal, fast agent runtime designed to run inside exo containers.
//! Provides LLM integration, memory, tools, and stdio communication.
//!
//! # Example
//!
//! ```no_run
//! use exo_agent::{ExoAgent, AgentConfig};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = AgentConfig::from_env()?;
//!     let mut agent = ExoAgent::new(config).await?;
//!     agent.run().await
//! }
//! ```

pub mod agent;
pub mod channel;
pub mod config;
pub mod llm;
pub mod memory;
pub mod tools;

pub use agent::{AgentState, ExoAgent};
pub use channel::{ChannelConfig, ChannelMode, InputMessage, OutputMessage, StdioChannel};
pub use config::{AgentConfig, LlmConfig, MemoryConfig, VolumeMount};
pub use llm::{ChatCompletion, FunctionCall, LlmClient, Message, Role, ToolCall, ToolDefinition};
pub use memory::{AgentMemory, Conversation};
pub use tools::{ToolName, ToolRegistry, ToolResult};

/// Current version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
