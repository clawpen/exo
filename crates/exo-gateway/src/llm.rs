//! LLM provider integration for local model inference
//!
//! Handles communication with Ollama/llama.cpp containers for chat completions,
//! embeddings, and model management.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Ollama API client for LLM operations
#[derive(Debug, Clone)]
pub struct LlmProvider {
    base_url: String,
    client: reqwest::Client,
}

impl LlmProvider {
    /// Create a new LLM provider
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("Failed to create HTTP client");
        
        Self { base_url, client }
    }
    
    /// Create default provider pointing to localhost Ollama
    pub fn default() -> Self {
        Self::new("http://localhost:11434")
    }
    
    /// Generate chat completion
    pub async fn chat(
        &self,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        let url = format!("{}/api/chat", self.base_url);
        
        debug!(
            model = %request.model,
            messages = request.messages.len(),
            "Sending chat request"
        );
        
        let ollama_req = OllamaChatRequest::from(request);
        
        let response = self.client
            .post(&url)
            .json(&ollama_req)
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError { status, message: text });
        }
        
        // Ollama returns NDJSON for streaming, but we'll collect it
        let body = response.text().await
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
        
        // Parse the final response from NDJSON
        let last_line = body.lines().last().unwrap_or("{}");
        let ollama_resp: OllamaChatResponse = serde_json::from_str(last_line)
            .map_err(|e| LlmError::ParseError(e.to_string()))?;
        
        Ok(ChatResponse::from(ollama_resp))
    }
    
    /// List available models
    pub async fn list_models(&self,
    ) -> Result<Vec<ModelInfo>, LlmError> {
        let url = format!("{}/api/tags", self.base_url);
        
        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError { status, message: text });
        }
        
        let tags: OllamaTagsResponse = response.json().await
            .map_err(|e| LlmError::ParseError(e.to_string()))?;
        
        Ok(tags.models.into_iter().map(ModelInfo::from).collect())
    }
    
    /// Pull a model from registry
    pub async fn pull_model(
        &self,
        name: &str,
    ) -> Result<PullProgress, LlmError> {
        let url = format!("{}/api/pull", self.base_url);
        
        info!(model = %name, "Pulling model");
        
        let req = serde_json::json!({
            "name": name,
            "stream": false
        });
        
        let response = self.client
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError { status, message: text });
        }
        
        let progress: OllamaPullResponse = response.json().await
            .map_err(|e| LlmError::ParseError(e.to_string()))?;
        
        Ok(PullProgress {
            completed: progress.completed,
            total: progress.total,
            status: progress.status,
        })
    }
    
    /// Generate embeddings
    pub async fn embeddings(
        &self,
        model: &str,
        text: &str,
    ) -> Result<Vec<f32>, LlmError> {
        let url = format!("{}/api/embeddings", self.base_url);
        
        let req = serde_json::json!({
            "model": model,
            "prompt": text
        });
        
        let response = self.client
            .post(&url)
            .json(&req)
            .send()
            .await
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError { status, message: text });
        }
        
        let embed: OllamaEmbedResponse = response.json().await
            .map_err(|e| LlmError::ParseError(e.to_string()))?;
        
        Ok(embed.embedding)
    }
    
    /// Check if Ollama is available
    pub async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        match self.client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }
}

// Request/Response types

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub tokens: TokenUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt: u32,
    pub completion: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub size: u64,
    pub modified: String,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullProgress {
    pub completed: u64,
    pub total: u64,
    pub status: String,
}

// Ollama API types (internal)

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<HashMap<String, serde_json::Value>>,
    stream: bool,
}

impl From<ChatRequest> for OllamaChatRequest {
    fn from(req: ChatRequest) -> Self {
        let mut options = HashMap::new();
        
        if let Some(temp) = req.temperature {
            options.insert("temperature".to_string(), temp.into());
        }
        
        if let Some(max_tokens) = req.max_tokens {
            options.insert("num_predict".to_string(), (max_tokens as i32).into());
        }
        
        Self {
            model: req.model,
            messages: req.messages.into_iter().map(Into::into).collect(),
            options: if options.is_empty() { None } else { Some(options) },
            stream: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

impl From<Message> for OllamaMessage {
    fn from(msg: Message) -> Self {
        Self {
            role: msg.role,
            content: msg.content,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    model: String,
    message: OllamaMessage,
    #[serde(default)]
    prompt_eval_count: u32,
    #[serde(default)]
    eval_count: u32,
    done: bool,
}

impl From<OllamaChatResponse> for ChatResponse {
    fn from(resp: OllamaChatResponse) -> Self {
        Self {
            content: resp.message.content,
            model: resp.model,
            tokens: TokenUsage {
                prompt: resp.prompt_eval_count,
                completion: resp.eval_count,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
    size: u64,
    modified_at: String,
    details: Option<OllamaModelDetails>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelDetails {
    parameter_size: Option<String>,
    quantization_level: Option<String>,
}

impl From<OllamaModel> for ModelInfo {
    fn from(m: OllamaModel) -> Self {
        Self {
            name: m.name,
            size: m.size,
            modified: m.modified_at,
            parameter_size: m.details.as_ref().and_then(|d| d.parameter_size.clone()),
            quantization: m.details.as_ref().and_then(|d| d.quantization_level.clone()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct OllamaPullResponse {
    status: String,
    #[serde(default)]
    completed: u64,
    #[serde(default)]
    total: u64,
}

#[derive(Debug, Deserialize)]
struct OllamaEmbedResponse {
    embedding: Vec<f32>,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Request failed: {0}")]
    RequestFailed(String),
    
    #[error("API error {status}: {message}")]
    ApiError { status: reqwest::StatusCode, message: String },
    
    #[error("Parse error: {0}")]
    ParseError(String),
    
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    
    #[error("GPU error: {0}")]
    GpuError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_chat_request_conversion() {
        let req = ChatRequest {
            model: "qwen2.5:0.5b".to_string(),
            messages: vec![
                Message { role: "user".to_string(), content: "Hello".to_string() }
            ],
            temperature: Some(0.7),
            max_tokens: Some(100),
        };
        
        let ollama_req: OllamaChatRequest = req.into();
        assert_eq!(ollama_req.model, "qwen2.5:0.5b");
        assert!(!ollama_req.stream);
        assert!(ollama_req.options.is_some());
    }
    
    #[tokio::test]
    async fn test_is_available_when_offline() {
        let provider = LlmProvider::new("http://localhost:19999");
        assert!(!provider.is_available().await);
    }
}
