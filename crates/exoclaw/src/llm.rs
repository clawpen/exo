//! LLM provider abstraction

use crate::{Error, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for LLM provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider type (openai_compatible, anthropic, ollama)
    pub provider: String,

    /// Base URL for API (e.g., http://localhost:1234/v1 for LM Studio)
    pub base_url: String,

    /// Model name
    pub model: String,

    /// API key (optional for local providers)
    pub api_key: Option<String>,
}

impl LlmConfig {
    /// Create config for LM Studio (OpenAI-compatible)
    pub fn lm_studio(model: String) -> Self {
        Self {
            provider: "openai_compatible".to_string(),
            base_url: "http://localhost:1234/v1".to_string(),
            model,
            api_key: None,
        }
    }

    /// Create config for Ollama
    pub fn ollama(model: String) -> Self {
        Self {
            provider: "ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            model,
            api_key: None,
        }
    }

    /// Create config for Claude
    pub fn anthropic(api_key: String, model: String) -> Self {
        Self {
            provider: "anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            model,
            api_key: Some(api_key),
        }
    }

    /// Create config for OpenAI
    pub fn openai(api_key: String, model: String) -> Self {
        Self {
            provider: "openai".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model,
            api_key: Some(api_key),
        }
    }
}

/// Request to LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub messages: Vec<LlmMessage>,
    pub tools: Option<Vec<ToolSpec>>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Response from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Trait for LLM providers - use async_trait for dyn-compatibility
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse>;
    fn config(&self) -> &LlmConfig;
}

/// OpenAI-compatible provider (works with LM Studio, Ollama, etc.)
pub struct OpenAiCompatibleProvider {
    config: LlmConfig,
    client: Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse> {
        let url = format!("{}/chat/completions", self.config.base_url);

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": request.messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "temperature": request.temperature.unwrap_or(0.7),
        });

        let mut req_builder = self.client.post(&url).json(&body);

        if let Some(api_key) = &self.config.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req_builder
            .send()
            .await
            .map_err(|e| Error::Llm(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("API error {}: {}", status, text)));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse response: {}", e)))?;

        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let finish_reason = json["choices"][0]["finish_reason"]
            .as_str()
            .map(|s| s.to_string());

        Ok(LlmResponse {
            content,
            tool_calls: None,
            finish_reason,
        })
    }

    fn config(&self) -> &LlmConfig {
        &self.config
    }
}

// Helper to convert to Arc<dyn LlmProvider>
pub fn into_provider(provider: OpenAiCompatibleProvider) -> Arc<dyn LlmProvider> {
    Arc::new(provider)
}
