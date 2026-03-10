//! Agent configuration.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent name
    pub name: String,
    
    /// LLM configuration
    pub llm: LlmConfig,
    
    /// Memory configuration
    pub memory: MemoryConfig,
    
    /// Channel configuration
    pub channel: ChannelConfig,
    
    /// System prompt
    pub system_prompt: Option<String>,
    
    /// Maximum tokens per response
    pub max_tokens: Option<u32>,
    
    /// Temperature (0.0 - 2.0)
    pub temperature: Option<f32>,
    
    /// Enabled tools
    pub tools: Vec<String>,
    
    /// Working directory
    pub workdir: PathBuf,
    
    /// Volume mounts (for file access)
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
}

/// LLM provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider: openai, anthropic, zai, ollama
    pub provider: String,
    
    /// API base URL (optional, uses defaults)
    pub base_url: Option<String>,
    
    /// API key (loaded from env or secrets)
    pub api_key: Option<String>,
    
    /// Model name
    pub model: String,
}

/// Memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Enable memory persistence
    pub enabled: bool,
    
    /// Database path (SQLite)
    pub db_path: PathBuf,
    
    /// Maximum conversation turns to keep
    pub max_turns: Option<usize>,
    
    /// Maximum tokens in context
    pub max_context_tokens: Option<usize>,
}

/// Channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Communication mode: stdio, websocket
    pub mode: ChannelMode,
    
    /// WebSocket URL (if mode is websocket)
    pub websocket_url: Option<String>,
}

/// Volume mount configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeMount {
    /// Source path on host (or within container)
    pub source: PathBuf,
    /// Target path inside container
    pub target: PathBuf,
    /// Mount as read-only
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelMode {
    Stdio,
    WebSocket,
}

impl AgentConfig {
    /// Load configuration from environment
    pub fn from_env() -> Result<Self> {
        let workdir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/workspace"));
        
        let config_path = workdir.join("config.toml");
        
        if config_path.exists() {
            Self::from_file(&config_path)
        } else {
            Self::default_config()
        }
    }
    
    /// Load configuration from file
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {:?}", path))?;
        
        let mut config: AgentConfig = toml::from_str(&content)
            .with_context(|| "Failed to parse config TOML")?;
        
        // Resolve API key from environment if not set
        if config.llm.api_key.is_none() {
            config.llm.api_key = Self::resolve_api_key(&config.llm.provider);
        }
        
        Ok(config)
    }
    
    /// Create default configuration
    pub fn default_config() -> Result<Self> {
        Ok(Self {
            name: "exo-agent".to_string(),
            llm: LlmConfig {
                provider: "zai".to_string(),
                base_url: Some("https://api.z.ai/api/coding/paas/v4".to_string()),
                api_key: std::env::var("ZAI_API_KEY").ok(),
                model: "glm-4.7-flash".to_string(),
            },
            memory: MemoryConfig {
                enabled: true,
                db_path: PathBuf::from("/data/memory.db"),
                max_turns: Some(50),
                max_context_tokens: Some(8192),
            },
            channel: ChannelConfig {
                mode: ChannelMode::Stdio,
                websocket_url: None,
            },
            system_prompt: None,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            tools: vec!["read".to_string(), "write".to_string(), "exec".to_string()],
            workdir: PathBuf::from("/workspace"),
            volumes: vec![],
        })
    }
    
    /// Resolve API key for a provider
    fn resolve_api_key(provider: &str) -> Option<String> {
        match provider {
            "openai" => std::env::var("OPENAI_API_KEY").ok(),
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            "zai" => std::env::var("ZAI_API_KEY").ok()
                .or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            "ollama" => None, // No key needed
            _ => None,
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            db_path: PathBuf::from("/data/memory.db"),
            max_turns: Some(50),
            max_context_tokens: Some(8192),
        }
    }
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            mode: ChannelMode::Stdio,
            websocket_url: None,
        }
    }
}
