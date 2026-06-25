//! Main agent implementation with autonomous tool calling.

use crate::channel::{InputMessage, OutputMessage, StdioChannel};
use crate::config::AgentConfig;
use crate::llm::{LlmClient, Message};
use anyhow::Result;
use tracing::{debug, info, instrument, warn};

#[cfg(feature = "memory")]
use crate::memory::AgentMemory;
#[cfg(feature = "tools")]
use crate::llm::ToolCall;
#[cfg(feature = "tools")]
use crate::tools::ToolRegistry;

/// Agent state
#[derive(Debug, Clone, Default)]
pub struct AgentState {
    pub running: bool,
    pub messages_processed: u64,
    pub tokens_used: u64,
    pub tool_calls_made: u64,
}

/// The agent
pub struct ExoAgent {
    config: AgentConfig,
    llm: LlmClient,
    #[cfg(feature = "memory")]
    memory: AgentMemory,
    #[cfg(not(feature = "memory"))]
    history: Vec<Message>,
    #[cfg(feature = "tools")]
    tools: ToolRegistry,
    channel: StdioChannel,
    state: AgentState,
}

impl ExoAgent {
    /// Create a new agent
    pub async fn new(config: AgentConfig) -> Result<Self> {
        let llm = LlmClient::new(config.llm.clone())?;
        #[cfg(feature = "memory")]
        let memory = AgentMemory::new(config.memory.clone()).await?;
        #[cfg(not(feature = "memory"))]
        let history = Vec::new();
        #[cfg(feature = "tools")]
        let tools = ToolRegistry::new();
        let channel = StdioChannel::new();

        Ok(Self {
            config,
            llm,
            #[cfg(feature = "memory")]
            memory,
            #[cfg(not(feature = "memory"))]
            history,
            #[cfg(feature = "tools")]
            tools,
            channel,
            state: AgentState::default(),
        })
    }

    /// Run the agent loop
    pub async fn run(&mut self) -> Result<()> {
        self.state.running = true;
        info!("Agent {} starting", self.config.name);

        // Add system prompt to context.
        if let Some(prompt) = self.config.system_prompt.clone() {
            self.add_message("system", &prompt).await?;
        }

        while self.state.running {
            // Receive message from channel
            match self.channel.recv().await? {
                Some(input) => {
                    self.handle_message(input).await?;
                }
                None => {
                    // Channel closed
                    break;
                }
            }
        }

        info!(
            "Agent stopped. Processed {} messages, {} tool calls, {} tokens used",
            self.state.messages_processed, self.state.tool_calls_made, self.state.tokens_used
        );
        Ok(())
    }

    /// Handle an incoming message with autonomous tool calling
    #[instrument(skip(self))]
    async fn handle_message(&mut self, input: InputMessage) -> Result<()> {
        debug!("Handling message: {:?}", input);

        // Add user message to context.
        self.add_message("user", &input.content).await?;
        self.state.messages_processed += 1;

        // Autonomous loop: think -> act -> observe. When compiled without the
        // `tools` feature, this runs exactly one LLM turn with no tool schema.
        let mut iteration = 0;
        let max_iterations = if cfg!(feature = "tools") { 10 } else { 1 };

        while iteration < max_iterations {
            iteration += 1;

            // Get context from memory/history
            let context = self.get_context(50).await?;

            // Call LLM with tools only when the binary was built with tool support.
            #[cfg(feature = "tools")]
            let tool_defs = Some(LlmClient::get_tool_definitions());
            #[cfg(not(feature = "tools"))]
            let tool_defs = None;

            let response = self
                .llm
                .complete(
                    context,
                    self.config.max_tokens,
                    self.config.temperature,
                    tool_defs,
                )
                .await?;

            // Track tokens
            if let Some(ref usage) = &response.usage {
                self.state.tokens_used += usage.total_tokens as u64;
            }

            // Extract assistant response
            let assistant_msg = response
                .choices
                .first()
                .map(|c| c.message.clone())
                .ok_or_else(|| anyhow::anyhow!("No response from LLM"))?;

            #[cfg(feature = "tools")]
            {
                // Check if we have tool calls
                if let Some(ref tool_calls) = assistant_msg.tool_calls {
                    if !tool_calls.is_empty() {
                        // Add assistant message with tool calls to context
                        self.add_message(
                            "assistant",
                            &format!("[Using {} tools]", tool_calls.len()),
                        )
                        .await?;

                        // Execute each tool call
                        for tool_call in tool_calls {
                            let result = self.execute_tool_call(tool_call).await?;

                            // Add tool result to context
                            self.add_message(
                                "tool",
                                &format!(
                                    "Tool {} returned: {}",
                                    tool_call.function.name, result
                                ),
                            )
                            .await?;

                            self.state.tool_calls_made += 1;
                        }

                        // Continue the loop - LLM will see tool results and decide next action
                        continue;
                    }
                }
            }

            // No tool calls - we have a final response
            self.add_message("assistant", &assistant_msg.content).await?;

            // Send response
            let output = OutputMessage {
                content: assistant_msg.content,
                tool_calls: None,
                done: Some(true),
            };
            self.channel.send(&output).await?;

            debug!("Response sent: {} chars (iteration {})", output.content.len(), iteration);
            return Ok(());
        }

        // Hit max iterations - send what we have
        warn!("Hit max iterations ({})", max_iterations);

        let output = OutputMessage {
            content: "I've completed the available actions but may need more iterations to finish. Please continue if needed.".to_string(),
            tool_calls: None,
            done: Some(false),
        };
        self.channel.send(&output).await?;

        Ok(())
    }

    #[cfg(feature = "memory")]
    async fn add_message(&mut self, role: &str, content: &str) -> Result<()> {
        self.memory.add(role, content).await?;
        Ok(())
    }

    #[cfg(not(feature = "memory"))]
    async fn add_message(&mut self, role: &str, content: &str) -> Result<()> {
        let role = match role {
            "system" => crate::llm::Role::System,
            "assistant" => crate::llm::Role::Assistant,
            "tool" => crate::llm::Role::Tool,
            _ => crate::llm::Role::User,
        };
        self.history.push(Message {
            role,
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
        Ok(())
    }

    #[cfg(feature = "memory")]
    async fn get_context(&self, limit: usize) -> Result<Vec<Message>> {
        self.memory.get_context(limit).await
    }

    #[cfg(not(feature = "memory"))]
    async fn get_context(&self, limit: usize) -> Result<Vec<Message>> {
        let start = self.history.len().saturating_sub(limit);
        Ok(self.history[start..].to_vec())
    }

    /// Execute a tool call from the LLM
    #[cfg(feature = "tools")]
    #[instrument(skip(self))]
    async fn execute_tool_call(&mut self, tool_call: &ToolCall) -> Result<String> {
        use anyhow::Context;

        let tool_name = &tool_call.function.name;
        let args_str = &tool_call.function.arguments;

        debug!("Executing tool: {}({})", tool_name, args_str);

        // Parse arguments
        let args: std::collections::HashMap<String, serde_json::Value> =
            serde_json::from_str(args_str)
                .with_context(|| format!("Failed to parse tool arguments: {}", args_str))?;

        // Execute via tool registry
        match self.tools.execute(tool_name, args).await {
            Ok(result) => {
                if result.success {
                    debug!("Tool {} succeeded", tool_name);
                    Ok(result.output)
                } else {
                    debug!("Tool {} failed: {:?}", tool_name, result.error);
                    Ok(format!(
                        "Error: {}",
                        result.error.unwrap_or_else(|| "Unknown error".to_string())
                    ))
                }
            }
            Err(e) => {
                warn!("Tool {} error: {}", tool_name, e);
                Ok(format!("Error: {}", e))
            }
        }
    }

    /// Stop the agent
    pub fn stop(&mut self) {
        self.state.running = false;
    }

    /// Get agent state
    pub fn state(&self) -> &AgentState {
        &self.state
    }
}
