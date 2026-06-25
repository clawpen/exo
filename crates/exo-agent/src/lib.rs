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
pub mod llm;
#[cfg(feature = "memory")]
pub mod memory;
#[cfg(feature = "tools")]
pub mod tools;
pub mod channel;
pub mod config;

pub use agent::{ExoAgent, AgentState};
pub use llm::{LlmClient, Message, Role, ChatCompletion, ToolCall, ToolDefinition, FunctionCall};
#[cfg(feature = "memory")]
pub use memory::{AgentMemory, Conversation};
#[cfg(feature = "tools")]
pub use tools::{ToolName, ToolRegistry, ToolResult};
pub use channel::{StdioChannel, InputMessage, OutputMessage, ChannelConfig, ChannelMode};
pub use config::{AgentConfig, LlmConfig, MemoryConfig, VolumeMount};

/// Current version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
