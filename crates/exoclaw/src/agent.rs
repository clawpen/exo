//! Agent runtime

use crate::llm::{LlmProvider, LlmRequest, LlmMessage};
use crate::protocol::ToolPermission;
use crate::tools::ToolRegistry;
use crate::{Result, SecurityEvent};
use std::sync::Arc;

/// Result of running an agent task
#[derive(Debug)]
pub struct AgentResult {
    pub success: bool,
    pub output: String,
    pub duration_ms: u64,
}

/// Main agent struct
pub struct Agent {
    pub id: String,
    pub name: String,
    pub llm: Arc<dyn LlmProvider>,
    pub tool_registry: Arc<ToolRegistry>,
}

impl Agent {
    pub fn new(
        name: String,
        llm: Arc<dyn LlmProvider>,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id,
            name,
            llm,
            tool_registry,
        }
    }

    /// Run a task synchronously (simplified version)
    pub fn run_task_sync(&self, task: String) -> Result<AgentResult> {
        let start = std::time::Instant::now();

        // Simple implementation - just call the LLM
        let rt = tokio::runtime::Runtime::new()?;
        let response = rt.block_on(self.llm.complete(LlmRequest {
            messages: vec![
                LlmMessage {
                    role: "system".to_string(),
                    content: "You are a helpful AI assistant with access to tools.".to_string(),
                },
                LlmMessage {
                    role: "user".to_string(),
                    content: task,
                },
            ],
            tools: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        }))?;

        Ok(AgentResult {
            success: true,
            output: response.content,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Check if a tool permission is granted
    pub fn check_permission(&self, tool_name: &str, permission: ToolPermission) -> bool {
        match permission {
            ToolPermission::None => true,
            ToolPermission::Never => false,
            ToolPermission::Once => {
                // Ask once per session (simplified - always yes for now)
                self.ask_permission(tool_name, "This session")
            }
            ToolPermission::Always => {
                // Ask every time
                self.ask_permission(tool_name, "this time")
            }
        }
    }

    fn ask_permission(&self, tool_name: &str, context: &str) -> bool {
        println!();
        println!("Permission Request:");
        println!("  Tool: {}", tool_name);
        println!("  Context: {}", context);
        print!("  Allow? [y/N]: ");
        use std::io::Write;
        std::io::stdout().flush().ok();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        let input = input.trim().to_lowercase();
        input == "y" || input == "yes"
    }

    /// Log a security event
    pub fn log_security(&self, event: SecurityEvent) {
        let timestamp = chrono::Local::now().to_rfc3339();
        println!("[{}] {} - {}: {}", timestamp, event.agent_id, event.event_type, event.details);
    }
}
