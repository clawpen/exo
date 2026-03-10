//! LLM client for chat completions with tool calling support.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

/// LLM client
pub struct LlmClient {
    client: Client,
    config: crate::config::LlmConfig,
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Tool call from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionCall,
}

/// Function call details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String, // JSON string
}

/// Tool definition for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

/// Function definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Chat completion request
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolDefinition>>,
    stream: bool,
}

/// Chat completion response
#[derive(Debug, Deserialize)]
pub struct ChatCompletion {
    pub id: String,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl LlmClient {
    /// Create a new LLM client
    pub fn new(config: crate::config::LlmConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to create HTTP client")?;
        
        Ok(Self { client, config })
    }
    
    /// Get the base URL for the provider
    fn base_url(&self) -> &str {
        self.config.base_url.as_deref()
            .unwrap_or_else(|| match self.config.provider.as_str() {
                "openai" => "https://api.openai.com/v1",
                "anthropic" => "https://api.anthropic.com/v1",
                "zai" => "https://api.z.ai/api/coding/paas/v4",
                "ollama" => "http://localhost:11434/v1",
                _ => "https://api.openai.com/v1",
            })
    }
    
    /// Get the auth header value
    fn auth_header(&self) -> Option<String> {
        self.config.api_key.as_ref().map(|key| {
            match self.config.provider.as_str() {
                "anthropic" => format!("x-api-key: {}", key),
                _ => format!("Bearer {}", key),
            }
        })
    }
    
    /// Get available tool definitions
    pub fn get_tool_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "read".to_string(),
                    description: "Read a file from the filesystem".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Path to the file to read"
                            }
                        },
                        "required": ["path"]
                    }),
                },
            },
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "write".to_string(),
                    description: "Write content to a file".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Path to the file to write"
                            },
                            "content": {
                                "type": "string",
                                "description": "Content to write to the file"
                            }
                        },
                        "required": ["path", "content"]
                    }),
                },
            },
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "exec".to_string(),
                    description: "Execute a shell command".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "Shell command to execute"
                            }
                        },
                        "required": ["command"]
                    }),
                },
            },
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "web_fetch".to_string(),
                    description: "Fetch content from a URL".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "url": {
                                "type": "string",
                                "description": "URL to fetch"
                            }
                        },
                        "required": ["url"]
                    }),
                },
            },
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "list".to_string(),
                    description: "List directory contents".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Directory path to list"
                            }
                        },
                        "required": ["path"]
                    }),
                },
            },
        ]
    }
    
    /// Complete a chat with optional tool calling
    #[instrument(skip(self, messages))]
    pub async fn complete(
        &self,
        messages: Vec<Message>,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> Result<ChatCompletion> {
        let url = format!("{}/chat/completions", self.base_url());
        
        let request = ChatRequest {
            model: self.config.model.clone(),
            messages,
            max_tokens,
            temperature,
            tools,
            stream: false,
        };
        
        debug!("Sending chat request to {}", url);
        
        let mut req = self.client.post(&url)
            .json(&request);
        
        if let Some(auth) = self.auth_header() {
            if auth.contains(':') {
                let parts: Vec<&str> = auth.splitn(2, ':').collect();
                req = req.header(parts[0], parts[1]);
            } else {
                req = req.header("Authorization", auth);
            }
        }
        
        let response = req.send()
            .await
            .context("Failed to send request to LLM API")?;
        
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM API error ({}): {}", status, error_text);
        }
        
        let completion: ChatCompletion = response.json()
            .await
            .context("Failed to parse LLM response")?;
        
        debug!("Chat completion received: {} tokens", 
            completion.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0));
        
        Ok(completion)
    }
    
    /// Simple chat helper (no tools)
    pub async fn chat(&self, system: Option<&str>, user: &str) -> Result<String> {
        let mut messages = Vec::new();
        
        if let Some(system) = system {
            messages.push(Message::system(system));
        }
        
        messages.push(Message::user(user));
        
        let completion = self.complete(messages, None, None, None).await?;
        
        completion.choices.first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| anyhow::anyhow!("No response from LLM"))
    }
}

impl Message {
    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    
    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    
    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    
    /// Create a tool result message
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}
