//! Tool system for agents.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, instrument};

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
        }
    }
    
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: String::new(),
            error: Some(error.into()),
        }
    }
}

/// Tool names
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    Read,
    Write,
    Exec,
    WebFetch,
    List,
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolName::Read => write!(f, "read"),
            ToolName::Write => write!(f, "write"),
            ToolName::Exec => write!(f, "exec"),
            ToolName::WebFetch => write!(f, "web_fetch"),
            ToolName::List => write!(f, "list"),
        }
    }
}

impl std::str::FromStr for ToolName {
    type Err = anyhow::Error;
    
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "read" => Ok(ToolName::Read),
            "write" => Ok(ToolName::Write),
            "exec" => Ok(ToolName::Exec),
            "web_fetch" => Ok(ToolName::WebFetch),
            "list" => Ok(ToolName::List),
            _ => anyhow::bail!("Unknown tool: {}", s),
        }
    }
}

/// Tool registry
pub struct ToolRegistry {
    enabled: Vec<ToolName>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            enabled: vec![ToolName::Read, ToolName::Write, ToolName::Exec, ToolName::WebFetch, ToolName::List],
        }
    }
    
    /// List available tools
    pub fn list(&self) -> Vec<&'static str> {
        self.enabled.iter().map(|t| match t {
            ToolName::Read => "read: Read a file from the filesystem",
            ToolName::Write => "write: Write content to a file",
            ToolName::Exec => "exec: Execute a shell command",
            ToolName::WebFetch => "web_fetch: Fetch content from a URL",
            ToolName::List => "list: List directory contents",
        }).collect()
    }
    
    /// Execute a tool
    #[instrument(skip(self, params))]
    pub async fn execute(&self, name: &str, params: HashMap<String, serde_json::Value>) -> Result<ToolResult> {
        let tool: ToolName = name.parse()?;
        
        match tool {
            ToolName::Read => read_file(&params).await,
            ToolName::Write => write_file(&params).await,
            ToolName::Exec => exec_command(&params).await,
            ToolName::WebFetch => web_fetch(&params).await,
            ToolName::List => list_dir(&params).await,
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tool Implementations
// ============================================================================

/// Read file tool
#[instrument(skip(params))]
async fn read_file(params: &HashMap<String, serde_json::Value>) -> Result<ToolResult> {
    let path = params.get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;
    
    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            // Truncate if too long
            let content = if content.len() > 10000 {
                format!("{}...\n[truncated, {} bytes total]", 
                    &content[..10000], content.len())
            } else {
                content
            };
            Ok(ToolResult::success(content))
        }
        Err(e) => Ok(ToolResult::error(format!("Failed to read file: {}", e))),
    }
}

/// Write file tool
#[instrument(skip(params))]
async fn write_file(params: &HashMap<String, serde_json::Value>) -> Result<ToolResult> {
    let path = params.get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' parameter"))?;
    
    let content = params.get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'content' parameter"))?;
    
    // Create parent directories if needed
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    
    match tokio::fs::write(path, content).await {
        Ok(()) => Ok(ToolResult::success(format!("Wrote {} bytes to {}", content.len(), path))),
        Err(e) => Ok(ToolResult::error(format!("Failed to write file: {}", e))),
    }
}

/// Execute command tool
#[instrument(skip(params))]
async fn exec_command(params: &HashMap<String, serde_json::Value>) -> Result<ToolResult> {
    let command = params.get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;
    
    // Security: block dangerous commands
    let dangerous = ["rm -rf", "sudo", "chmod 777", "> /dev/"];
    if dangerous.iter().any(|d| command.contains(d)) {
        return Ok(ToolResult::error("Command blocked for security"));
    }
    
    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .await;
    
    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            
            let result = if output.status.success() {
                ToolResult::success(if stderr.is_empty() {
                    stdout.to_string()
                } else {
                    format!("{}\n[stderr: {}]", stdout, stderr)
                })
            } else {
                ToolResult::error(format!("Exit {}: {}", output.status, stderr))
            };
            Ok(result)
        }
        Err(e) => Ok(ToolResult::error(format!("Failed to execute: {}", e))),
    }
}

/// Web fetch tool
#[instrument(skip(params))]
async fn web_fetch(params: &HashMap<String, serde_json::Value>) -> Result<ToolResult> {
    let url = params.get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'url' parameter"))?;
    
    // Validate URL
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Ok(ToolResult::error("Invalid URL scheme"));
    }
    
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("exo-agent/0.1")
        .build()
        .context("Failed to create HTTP client")?;
    
    match client.get(url).send().await {
        Ok(response) => {
            if !response.status().is_success() {
                return Ok(ToolResult::error(format!("HTTP {}", response.status())));
            }
            
            match response.text().await {
                Ok(text) => {
                    // Truncate if too long
                    let text = if text.len() > 10000 {
                        format!("{}...\n[truncated, {} bytes total]", 
                            &text[..10000], text.len())
                    } else {
                        text
                    };
                    Ok(ToolResult::success(text))
                }
                Err(e) => Ok(ToolResult::error(format!("Failed to read response: {}", e))),
            }
        }
        Err(e) => Ok(ToolResult::error(format!("Request failed: {}", e))),
    }
}

/// List directory tool
#[instrument(skip(params))]
async fn list_dir(params: &HashMap<String, serde_json::Value>) -> Result<ToolResult> {
    let path = params.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    
    let mut entries = vec![];
    
    let mut dir = match tokio::fs::read_dir(path).await {
        Ok(d) => d,
        Err(e) => return Ok(ToolResult::error(format!("Failed to read directory: {}", e))),
    };
    
    while let Ok(Some(entry)) = dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.metadata().await.ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        
        entries.push(if is_dir {
            format!("{}/", name)
        } else {
            format!("{} ({})", name, size)
        });
    }
    
    entries.sort();
    Ok(ToolResult::success(entries.join("\n")))
}
