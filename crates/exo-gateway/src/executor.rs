//! Tool execution bridge between gateway and exo-runtime
//!
//! This module connects the skill registry to actual container execution,
//! handling the lifecycle of tool invocations.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use crate::llm::{ChatRequest, LlmProvider, Message};
use crate::protocol::{RequestId, ToolResult};
use crate::skill::{SkillRegistry, SkillRuntime, SkillError};

/// Manages execution of tools via containers or other runtimes
#[derive(Debug, Clone)]
pub struct ToolExecutor {
    registry: Arc<SkillRegistry>,
    /// Container image cache/location
    container_storage: PathBuf,
    /// Default timeout for tool execution
    default_timeout: Duration,
    /// Track running executions
    running: Arc<tokio::sync::RwLock<HashMap<String, RunningExecution>>>,
    /// LLM provider for direct API calls
    llm_provider: Option<LlmProvider>,
}

#[derive(Debug)]
struct RunningExecution {
    skill: String,
    tool: String,
    started_at: std::time::Instant,
    abort_tx: tokio::sync::oneshot::Sender<()>,
}

/// Result of a tool execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
}

impl ToolExecutor {
    pub fn new(
        registry: Arc<SkillRegistry>,
        container_storage: impl Into<PathBuf>,
    ) -> Self {
        Self {
            registry,
            container_storage: container_storage.into(),
            default_timeout: Duration::from_secs(30),
            running: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            llm_provider: Some(LlmProvider::default()),
        }
    }

    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub fn with_llm_provider(mut self, provider: LlmProvider) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    /// Execute a tool with the given arguments
    pub async fn execute(
        &self,
        request_id: String,
        skill_name: &str,
        tool_name: &str,
        args: serde_json::Value,
        timeout_ms: Option<u64>,
    ) -> Result<ToolResult, ExecutionError> {
        let start = std::time::Instant::now();
        
        // Look up the skill
        let skill = self.registry.get(skill_name).await
            .ok_or_else(|| ExecutionError::SkillNotFound(skill_name.to_string()))?;
        
        // Find the tool definition
        let tool_def = skill.manifest.tools.iter()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| ExecutionError::ToolNotFound(tool_name.to_string()))?;
        
        info!(
            request_id = %request_id,
            skill = %skill_name,
            tool = %tool_name,
            "Executing tool"
        );

        let timeout_duration = timeout_ms
            .map(|ms| Duration::from_millis(ms))
            .or(tool_def.timeout_ms.map(|ms| Duration::from_millis(ms)))
            .unwrap_or(self.default_timeout);

        // Create abort channel for cancellation
        let (abort_tx, abort_rx) = tokio::sync::oneshot::channel();
        
        // Track running execution
        let running_exec = RunningExecution {
            skill: skill_name.to_string(),
            tool: tool_name.to_string(),
            started_at: start,
            abort_tx,
        };
        self.running.write().await.insert(request_id.clone(), running_exec);

        // Execute based on runtime type
        let result = if skill_name == "llm" {
            // Special handling for LLM skill - use HTTP API
            if let Some(ref provider) = self.llm_provider {
                self.execute_llm_tool(tool_name, args, provider).await
            } else {
                Ok(ExecutionResult {
                    stdout: serde_json::json!({"error": "LLM provider not configured"}).to_string(),
                    stderr: "LLM provider not available".to_string(),
                    exit_code: 1,
                    execution_time_ms: 0,
                })
            }
        } else {
            match &skill.manifest.runtime {
                SkillRuntime::Container { image, resources, env } => {
                    self.execute_container(
                        skill_name,
                        tool_name,
                        args,
                        image,
                        resources.memory.clone(),
                        resources.cpu,
                        resources.gpu,
                        env.clone(),
                        abort_rx,
                    ).await
                }
                SkillRuntime::Wasm { module, memory_limit_mb } => {
                    self.execute_wasm(
                        skill_name,
                        tool_name,
                        args,
                        module,
                        *memory_limit_mb,
                        abort_rx,
                    ).await
                }
                SkillRuntime::Builtin => {
                    self.execute_builtin(
                        skill_name,
                        tool_name,
                        args,
                        abort_rx,
                    ).await
                }
            }
        };

        // Remove from running
        self.running.write().await.remove(&request_id);

        let execution_time_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(exec_result) => {
                info!(
                    request_id = %request_id,
                    exit_code = exec_result.exit_code,
                    execution_time_ms,
                    "Tool execution completed"
                );

                if exec_result.exit_code == 0 {
                    // Try to parse stdout as JSON
                    let output = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&exec_result.stdout) {
                        json
                    } else {
                        serde_json::json!({ "output": exec_result.stdout.trim() })
                    };

                    Ok(ToolResult::Success {
                        output,
                        execution_time_ms,
                    })
                } else {
                    Ok(ToolResult::Error {
                        code: format!("EXIT_{}", exec_result.exit_code),
                        message: exec_result.stderr.clone(),
                    })
                }
            }
            Err(e) => {
                error!(
                    request_id = %request_id,
                    error = %e,
                    "Tool execution failed"
                );
                
                if e.is_timeout() {
                    Ok(ToolResult::Timeout {
                        timeout_ms: timeout_duration.as_millis() as u64,
                    })
                } else {
                    Ok(ToolResult::Error {
                        code: "EXECUTION_FAILED".to_string(),
                        message: e.to_string(),
                    })
                }
            }
        }
    }

    /// Execute tool in a container using exo-runtime
    async fn execute_container(
        &self,
        skill: &str,
        tool: &str,
        args: serde_json::Value,
        image: &str,
        memory: String,
        cpu: f32,
        gpu: bool,
        env: HashMap<String, String>,
        mut abort_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<ExecutionResult, ExecutionError> {
        // For now, use docker/podman as the container runtime
        // In the future, this will use exo-runtime directly
        
        let tool_input = serde_json::to_string(&args)?;
        
        // Build container name
        let container_name = format!("exo-skill-{}-{}", skill, tool);
        
        // Check if we should use podman or docker
        let runtime = if Self::has_podman().await {
            "podman"
        } else if Self::has_docker().await {
            "docker"
        } else {
            return Err(ExecutionError::NoContainerRuntime);
        };

        debug!(
            runtime,
            image,
            container_name,
            "Spawning container"
        );

        // Build the command
        let mut cmd = Command::new(runtime);
        cmd.arg("run")
            .arg("--rm")
            .arg("--name").arg(&container_name)
            .arg("--memory").arg(&memory)
            .arg("--cpus").arg(cpu.to_string())
            .arg("--network").arg("none"); // Secure by default - no network

        // Add GPU if requested
        if gpu {
            cmd.arg("--gpus").arg("all");
        }

        // Add environment variables
        for (key, value) in env {
            cmd.arg("-e").arg(format!("{}={}", key, value));
        }

        // Add the image and command
        cmd.arg(image);
        
        // Pass args via stdin (tool reads JSON from stdin)
        cmd.stdin(std::process::Stdio::piped())
           .stdout(std::process::Stdio::piped())
           .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()
            .map_err(|e| ExecutionError::SpawnFailed(e.to_string()))?;

        // Write args to stdin
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(tool_input.as_bytes()).await.ok();
        }

        // Create a task to wait for the child process
        let child_handle = tokio::spawn(async move {
            child.wait_with_output().await
        });

        // Wait for completion or abort
        let result = tokio::select! {
            output = child_handle => {
                match output {
                    Ok(Ok(output)) => Ok(ExecutionResult {
                        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                        exit_code: output.status.code().unwrap_or(-1),
                        execution_time_ms: 0, // Calculated by caller
                    }),
                    Ok(Err(e)) => Err(ExecutionError::ExecutionFailed(e.to_string())),
                    Err(e) => Err(ExecutionError::ExecutionFailed(e.to_string())),
                }
            }
            _ = abort_rx => {
                // Cancel the wait task - child will be dropped and killed
                Err(ExecutionError::Cancelled)
            }
        };

        result
    }

    /// Execute WASM module (placeholder for future implementation)
    async fn execute_wasm(
        &self,
        _skill: &str,
        _tool: &str,
        args: serde_json::Value,
        module: &str,
        _memory_limit_mb: u32,
        _abort_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<ExecutionResult, ExecutionError> {
        // WASM execution would use wasmtime or similar
        // For now, return a not-implemented error
        
        warn!(module, "WASM execution not yet implemented");
        
        Ok(ExecutionResult {
            stdout: serde_json::to_string(&args)?,
            stderr: "WASM runtime not yet implemented".to_string(),
            exit_code: 1,
            execution_time_ms: 0,
        })
    }

    /// Execute LLM tool via HTTP API
    async fn execute_llm_tool(
        &self,
        tool: &str,
        args: serde_json::Value,
        provider: &LlmProvider,
    ) -> Result<ExecutionResult, ExecutionError> {
        let start = std::time::Instant::now();
        
        let result = match tool {
            "chat" => {
                // Parse chat request from args
                let model = args.get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("qwen2.5:0.5b")
                    .to_string();
                
                let messages: Vec<Message> = args.get("messages")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                let role = m.get("role")?.as_str()?.to_string();
                                let content = m.get("content")?.as_str()?.to_string();
                                Some(Message { role, content })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                
                let temperature = args.get("temperature").and_then(|v| v.as_f64()).map(|f| f as f32);
                let max_tokens = args.get("max_tokens").and_then(|v| v.as_u64()).map(|u| u as u32);
                
                let request = ChatRequest {
                    model,
                    messages,
                    temperature,
                    max_tokens,
                };
                
                match provider.chat(request).await {
                    Ok(response) => {
                        let output = serde_json::json!({
                            "content": response.content,
                            "model": response.model,
                            "tokens": {
                                "prompt": response.tokens.prompt,
                                "completion": response.tokens.completion
                            }
                        });
                        Ok(ExecutionResult {
                            stdout: output.to_string(),
                            stderr: String::new(),
                            exit_code: 0,
                            execution_time_ms: start.elapsed().as_millis() as u64,
                        })
                    }
                    Err(e) => {
                        Err(ExecutionError::ExecutionFailed(e.to_string()))
                    }
                }
            }
            
            "list_models" => {
                match provider.list_models().await {
                    Ok(models) => {
                        let output = serde_json::json!({
                            "models": models.iter().map(|m| {
                                serde_json::json!({
                                    "name": m.name,
                                    "size": m.size,
                                    "modified": m.modified,
                                    "parameter_size": m.parameter_size,
                                    "quantization": m.quantization
                                })
                            }).collect::<Vec<_>>()
                        });
                        Ok(ExecutionResult {
                            stdout: output.to_string(),
                            stderr: String::new(),
                            exit_code: 0,
                            execution_time_ms: start.elapsed().as_millis() as u64,
                        })
                    }
                    Err(e) => {
                        Err(ExecutionError::ExecutionFailed(e.to_string()))
                    }
                }
            }
            
            "pull" => {
                let model = args.get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                
                if model.is_empty() {
                    return Ok(ExecutionResult {
                        stdout: serde_json::json!({"error": "model parameter required"}).to_string(),
                        stderr: "Missing model parameter".to_string(),
                        exit_code: 1,
                        execution_time_ms: 0,
                    });
                }
                
                match provider.pull_model(&model).await {
                    Ok(progress) => {
                        let output = serde_json::json!({
                            "status": progress.status,
                            "completed": progress.completed,
                            "total": progress.total
                        });
                        Ok(ExecutionResult {
                            stdout: output.to_string(),
                            stderr: String::new(),
                            exit_code: 0,
                            execution_time_ms: start.elapsed().as_millis() as u64,
                        })
                    }
                    Err(e) => {
                        Err(ExecutionError::ExecutionFailed(e.to_string()))
                    }
                }
            }
            
            "embeddings" => {
                let model = args.get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("nomic-embed-text")
                    .to_string();
                let text = args.get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                
                match provider.embeddings(&model, &text).await {
                    Ok(embeddings) => {
                        let output = serde_json::json!({
                            "embeddings": embeddings
                        });
                        Ok(ExecutionResult {
                            stdout: output.to_string(),
                            stderr: String::new(),
                            exit_code: 0,
                            execution_time_ms: start.elapsed().as_millis() as u64,
                        })
                    }
                    Err(e) => {
                        Err(ExecutionError::ExecutionFailed(e.to_string()))
                    }
                }
            }
            
            _ => {
                Err(ExecutionError::ToolNotFound(format!("llm:{}", tool)))
            }
        };
        
        result
    }

    /// Execute builtin tool (placeholder for future implementation)
    async fn execute_builtin(
        &self,
        _skill: &str,
        tool: &str,
        args: serde_json::Value,
        _abort_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<ExecutionResult, ExecutionError> {
        // Builtin tools would be Rust functions compiled into the binary
        // For now, support a few basic tools
        
        match tool {
            "echo" => {
                let output = serde_json::to_string_pretty(&args)?;
                Ok(ExecutionResult {
                    stdout: output,
                    stderr: String::new(),
                    exit_code: 0,
                    execution_time_ms: 0,
                })
            }
            "time" => {
                let now = chrono::Local::now();
                Ok(ExecutionResult {
                    stdout: serde_json::json!({
                        "timestamp": now.timestamp(),
                        "iso": now.to_rfc3339(),
                        "local": now.to_string(),
                    }).to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                    execution_time_ms: 0,
                })
            }
            _ => {
                Err(ExecutionError::ToolNotFound(tool.to_string()))
            }
        }
    }

    /// Cancel a running execution
    pub async fn cancel(&self, request_id: &str) -> bool {
        if let Some(exec) = self.running.write().await.remove(request_id) {
            let _ = exec.abort_tx.send(());
            info!(request_id = %request_id, "Execution cancelled");
            true
        } else {
            false
        }
    }

    /// Check if an execution is running
    pub async fn is_running(&self, request_id: &str) -> bool {
        self.running.read().await.contains_key(request_id)
    }

    /// List running executions
    pub async fn list_running(&self) -> Vec<(String, String, String, std::time::Duration)> {
        let now = std::time::Instant::now();
        let running = self.running.read().await;
        running
            .iter()
            .map(|(id, exec)| {
                let duration = now - exec.started_at;
                (id.clone(), exec.skill.clone(), exec.tool.clone(), duration)
            })
            .collect()
    }

    /// Check if podman is available
    async fn has_podman() -> bool {
        Command::new("podman")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Check if docker is available
    async fn has_docker() -> bool {
        Command::new("docker")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new(
            Arc::new(SkillRegistry::new()),
            PathBuf::from("/var/lib/openclaw/skills"),
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("Skill not found: {0}")]
    SkillNotFound(String),
    
    #[error("Tool not found: {0}")]
    ToolNotFound(String),
    
    #[error("No container runtime available (docker/podman)")]
    NoContainerRuntime,
    
    #[error("Failed to spawn container: {0}")]
    SpawnFailed(String),
    
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    
    #[error("Execution timed out")]
    Timeout,
    
    #[error("Execution cancelled")]
    Cancelled,
    
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Skill error: {0}")]
    Skill(#[from] SkillError),
}

impl ExecutionError {
    fn is_timeout(&self) -> bool {
        matches!(self, ExecutionError::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_builtin_echo() {
        let executor = ToolExecutor::default();
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        let result = executor.execute_builtin(
            "test",
            "echo",
            serde_json::json!({"message": "hello"}),
            rx,
        ).await;
        
        assert!(result.is_ok());
        let exec_result = result.unwrap();
        assert_eq!(exec_result.exit_code, 0);
        assert!(exec_result.stdout.contains("hello"));
    }

    #[tokio::test]
    async fn test_builtin_time() {
        let executor = ToolExecutor::default();
        let (tx, rx) = tokio::sync::oneshot::channel();
        
        let result = executor.execute_builtin(
            "test",
            "time",
            serde_json::Value::Null,
            rx,
        ).await;
        
        assert!(result.is_ok());
        let exec_result = result.unwrap();
        assert_eq!(exec_result.exit_code, 0);
        assert!(exec_result.stdout.contains("timestamp"));
    }
}
