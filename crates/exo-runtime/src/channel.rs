//! Agent communication channel for AI agent containers.
//!
//! This module implements the communication protocol between the host
//! and AI agents running inside containers. It provides:
//!
//! - Stdio pipes for control messages and observations
//! - Tool bus for safe tool invocation
//! - Structured message types for agent communication
//!
//! # Architecture
//!
//! ```text
//! Host                                     Container
//!   │                                         │
//!   │  ┌─────────────────────────────────┐  │
//!   │  │         AgentChannel            │  │
//!   │  │                                 │  │
//!   │  │  stdin  ────→  Control Messages │  │
//!   │  │  stdout ←────  Observations     │  │
//!   │  │  stderr ←────  Errors/Logs      │  │
//!   │  │                                 │  │
//!   │  │  tool_bus ───→ Tool Requests   │  │
//!   │  │              ←── Tool Responses │  │
//!   │  └─────────────────────────────────┘  │
//!   │                                         │
//! ┌─▼───┐                                 ┌──▼──┐
//! │ Host │                                 │Agent│
//! └─────┘                                 └─────┘
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use uuid::Uuid;

/// Default timeout for tool execution (in seconds).
pub const DEFAULT_TOOL_TIMEOUT: u64 = 30;

/// Maximum message size for agent communication.
pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024; // 10 MB

/// Communication channel between host and AI agent.
#[derive(Clone)]
pub struct AgentChannel {
    /// Channel ID
    pub id: String,

    /// Container name/ID
    pub container_id: String,

    /// Stdin pipe for sending control messages
    stdin: Arc<Mutex<Box<dyn Write + Send>>>,

    /// Stdout pipe for reading observations
    stdout: Arc<Mutex<Box<dyn BufRead + Send>>>,

    /// Stderr pipe for reading errors
    stderr: Arc<Mutex<Box<dyn BufRead + Send>>>,

    /// Tool bus callback for executing tools
    tool_executor: Arc<Mutex<Box<dyn ToolExecutor>>>,

    /// Message callback for observations
    on_observation: Arc<Mutex<Box<dyn Fn(AgentMessage) + Send>>>,

    /// Message callback for errors
    pub on_error: Arc<Mutex<Box<dyn Fn(String) + Send>>>,

    /// Whether the channel is active
    active: Arc<Mutex<bool>>,
}

/// Message sent from host to agent or vice versa.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Message ID
    pub id: String,

    /// Message type
    #[serde(rename = "type")]
    pub message_type: MessageType,

    /// Message content
    pub content: String,

    /// Optional metadata
    #[serde(default)]
    pub metadata: serde_json::Value,

    /// Timestamp
    #[serde(default)]
    pub timestamp: i64,
}

/// Type of agent message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    /// Control message (host → agent)
    Control,

    /// Observation (agent → host)
    Observation,

    /// Tool request (agent → host)
    ToolRequest,

    /// Tool response (host → agent)
    ToolResponse,

    /// Error message
    Error,

    /// Status/heartbeat
    Status,
}

/// Tool request from the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    /// Request ID
    pub id: String,

    /// Tool name
    pub tool: String,

    /// Tool arguments
    pub arguments: serde_json::Value,

    /// Optional timeout in seconds
    pub timeout: Option<u64>,

    /// Working directory
    pub workdir: Option<PathBuf>,
}

/// Tool response to the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    /// Request ID this responds to
    pub request_id: String,

    /// Exit code (0 = success)
    pub exit_code: i32,

    /// Standard output
    pub stdout: String,

    /// Standard error
    pub stderr: String,

    /// Whether the tool timed out
    pub timed_out: bool,

    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Tool definition for sandboxed execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool name
    pub name: String,

    /// Description
    pub description: String,

    /// Command to run
    pub command: String,

    /// Arguments schema (JSON Schema)
    pub arguments_schema: Option<serde_json::Value>,

    /// Whether this tool requires network access
    pub requires_network: bool,

    /// Whether this tool requires file system access
    pub requires_fs: bool,

    /// Maximum runtime in seconds
    pub max_timeout: u64,
}

impl ToolDef {
    /// Create a new tool definition.
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            command: command.into(),
            arguments_schema: None,
            requires_network: false,
            requires_fs: false,
            max_timeout: DEFAULT_TOOL_TIMEOUT,
        }
    }

    /// Set the description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set whether network access is required.
    pub fn requires_network(mut self, requires: bool) -> Self {
        self.requires_network = requires;
        self
    }

    /// Set whether filesystem access is required.
    pub fn requires_fs(mut self, requires: bool) -> Self {
        self.requires_fs = requires;
        self
    }

    /// Set maximum timeout.
    pub fn max_timeout(mut self, timeout: u64) -> Self {
        self.max_timeout = timeout;
        self
    }
}

impl AgentChannel {
    /// Create a new agent channel for a container process.
    pub fn new(
        container_id: impl Into<String>,
        stdin: Box<dyn Write + Send>,
        stdout: Box<dyn BufRead + Send>,
        stderr: Box<dyn BufRead + Send>,
        tool_executor: Box<dyn ToolExecutor>,
    ) -> Self {
        let id = Uuid::new_v4().to_string();

        Self {
            id,
            container_id: container_id.into(),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(stdout)),
            stderr: Arc::new(Mutex::new(stderr)),
            tool_executor: Arc::new(Mutex::new(tool_executor)),
            on_observation: Arc::new(Mutex::new(Box::new(|_| {}))),
            on_error: Arc::new(Mutex::new(Box::new(|_| {}))),
            active: Arc::new(Mutex::new(true)),
        }
    }

    /// Set callback for observations.
    pub fn on_observation<F: Fn(AgentMessage) + Send + 'static>(mut self, f: F) -> Self {
        self.on_observation = Arc::new(Mutex::new(Box::new(f)));
        self
    }

    /// Set callback for errors.
    pub fn on_error<F: Fn(String) + Send + 'static>(mut self, f: F) -> Self {
        self.on_error = Arc::new(Mutex::new(Box::new(f)));
        self
    }

    /// Start the message reader loops.
    pub fn start(&self) -> Result<()> {
        let stdout = self.stdout.clone();
        let stderr = self.stderr.clone();
        let on_observation = self.on_observation.clone();
        let on_error = self.on_error.clone();
        let active = self.active.clone();

        // Spawn stdout reader
        thread::spawn(move || {
            let mut reader = stdout.lock().unwrap();
            let mut buffer = String::new();
            loop {
                buffer.clear();
                match reader.read_line(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(msg) = serde_json::from_str::<AgentMessage>(&buffer) {
                            on_observation.lock().unwrap()(msg);
                        }
                    }
                    Err(_) => break,
                }
            }
            *active.lock().unwrap() = false;
        });

        // Spawn stderr reader
        thread::spawn(move || {
            let mut reader = stderr.lock().unwrap();
            let mut buffer = String::new();
            loop {
                buffer.clear();
                match reader.read_line(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) if n > 0 => {
                        let line = buffer.trim().to_string();
                        on_error.lock().unwrap()(line);
                    }
                    Err(_) => break,
                    _ => {}
                }
            }
        });

        Ok(())
    }

    /// Send a control message to the agent.
    pub fn send_control(&self, content: impl Into<String>) -> Result<()> {
        let msg = AgentMessage {
            id: Uuid::new_v4().to_string(),
            message_type: MessageType::Control,
            content: content.into(),
            metadata: serde_json::json!({}),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        };

        self.send_message(msg)
    }

    /// Send a tool response to the agent.
    pub fn send_tool_response(&self, response: ToolResponse) -> Result<()> {
        let msg = AgentMessage {
            id: Uuid::new_v4().to_string(),
            message_type: MessageType::ToolResponse,
            content: serde_json::to_string(&response)?,
            metadata: serde_json::json!({
                "request_id": response.request_id,
                "exit_code": response.exit_code,
            }),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        };

        self.send_message(msg)
    }

    /// Send a message to the agent.
    fn send_message(&self, msg: AgentMessage) -> Result<()> {
        let json = serde_json::to_string(&msg)?;
        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{}", json)?;
        stdin.flush()?;
        Ok(())
    }

    /// Check if the channel is active.
    pub fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }

    /// Get the channel ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Close the channel.
    pub fn close(&self) -> Result<()> {
        *self.active.lock().unwrap() = false;
        Ok(())
    }
}

/// Tool executor for running commands in a subprocess.
pub trait ToolExecutor: Send {
    /// Execute a tool request and return the response.
    fn execute(&self, request: ToolRequest) -> ToolResponse;
}

/// Default tool executor that runs commands in a subprocess.
pub struct DefaultToolExecutor {
    /// Allowed tools
    allowed_tools: Vec<ToolDef>,

    /// Working directory for tool execution
    workdir: PathBuf,

    /// Environment variables for tools
    env: Vec<(String, String)>,
}

impl DefaultToolExecutor {
    /// Create a new tool executor.
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            allowed_tools: Vec::new(),
            workdir: workdir.into(),
            env: Vec::new(),
        }
    }

    /// Add an allowed tool.
    pub fn add_tool(mut self, tool: ToolDef) -> Self {
        self.allowed_tools.push(tool);
        self
    }

    /// Add multiple allowed tools.
    pub fn add_tools(mut self, tools: Vec<ToolDef>) -> Self {
        for tool in tools {
            self.allowed_tools.push(tool);
        }
        self
    }

    /// Set environment variables.
    pub fn env(mut self, vars: Vec<(String, String)>) -> Self {
        self.env = vars;
        self
    }

    /// Find a tool by name.
    fn find_tool(&self, name: &str) -> Option<&ToolDef> {
        self.allowed_tools.iter().find(|t| t.name == name)
    }
}

impl ToolExecutor for DefaultToolExecutor {
    fn execute(&self, request: ToolRequest) -> ToolResponse {
        let start = std::time::Instant::now();

        // Find the tool definition
        let tool_def = match self.find_tool(&request.tool) {
            Some(t) => t,
            None => {
                return ToolResponse {
                    request_id: request.id,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Tool not found: {}", request.tool),
                    timed_out: false,
                    duration_ms: 0,
                };
            }
        };

        // Parse arguments into command-line args
        let mut args = Vec::new();
        if let serde_json::Value::Object(obj) = &request.arguments {
            for (k, v) in obj {
                let flag = format!("--{}", k);
                args.push(flag);

                if let serde_json::Value::String(s) = v {
                    args.push(s.clone());
                } else if let serde_json::Value::Bool(b) = v {
                    args.push(b.to_string());
                } else if let serde_json::Value::Number(n) = v {
                    args.push(n.to_string());
                } else {
                    args.push(v.to_string());
                }
            }
        }

        // Build the command
        let mut cmd = Command::new(&tool_def.command);
        cmd.args(&args);

        // Set working directory
        let workdir = request.workdir.as_ref().unwrap_or(&self.workdir);
        cmd.current_dir(workdir);

        // Set environment
        for (k, v) in &self.env {
            cmd.env(k, v);
        }

        // Capture output
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Execute with timeout
        let timeout = request.timeout.unwrap_or(tool_def.max_timeout);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ToolResponse {
                    request_id: request.id,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Failed to spawn: {}", e),
                    timed_out: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        // Wait for completion or timeout
        let duration = std::time::Duration::from_secs(timeout);
        let result = match wait_with_timeout(&mut child, duration) {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .unwrap_or_else(|_| std::process::Output {
                        status: status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    });

                ToolResponse {
                    request_id: request.id,
                    exit_code: output.status.code().unwrap_or(1),
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    timed_out: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }
            Ok(None) => {
                // Timeout - kill the child process
                let _ = child.kill();
                ToolResponse {
                    request_id: request.id,
                    exit_code: 124,
                    stdout: String::new(),
                    stderr: format!("Tool timed out after {} seconds", timeout),
                    timed_out: true,
                    duration_ms: duration.as_millis() as u64,
                }
            }
            Err(e) => ToolResponse {
                request_id: request.id,
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("Error executing tool: {}", e),
                timed_out: false,
                duration_ms: start.elapsed().as_millis() as u64,
            },
        };

        result
    }
}

/// Wait for process with timeout.
fn wait_with_timeout(
    child: &mut std::process::Child,
    duration: std::time::Duration,
) -> Result<Option<std::process::ExitStatus>, std::io::Error> {
    let start = std::time::Instant::now();

    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Ok(Some(status));
        }

        if start.elapsed() >= duration {
            return Ok(None);
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Create a channel from a spawned container process.
#[cfg(target_os = "linux")]
pub fn create_channel_for_process(
    container_id: &str,
    process: &std::process::Child,
    tool_executor: Box<dyn ToolExecutor>,
) -> Result<AgentChannel> {
    // TODO: Extract file descriptors from the process
    Err(anyhow::anyhow!("Not yet implemented"))
}

/// Standard tool definitions for common AI agent operations.
pub fn standard_tools() -> Vec<ToolDef> {
    vec![
        ToolDef::new("read_file", "/bin/cat")
            .description("Read the contents of a file")
            .requires_fs(true),
        ToolDef::new("write_file", "/usr/bin/tee")
            .description("Write content to a file")
            .requires_fs(true),
        ToolDef::new("list_files", "/bin/ls")
            .description("List files in a directory")
            .requires_fs(true),
        ToolDef::new("http_get", "/usr/bin/curl")
            .description("Make an HTTP GET request")
            .requires_network(true),
        ToolDef::new("grep", "/bin/grep")
            .description("Search for patterns in text")
            .requires_fs(true),
        ToolDef::new("head", "/usr/bin/head")
            .description("Show the beginning of a file")
            .requires_fs(true),
        ToolDef::new("tail", "/usr/bin/tail")
            .description("Show the end of a file")
            .requires_fs(true),
        ToolDef::new("wc", "/usr/bin/wc")
            .description("Count lines, words, and bytes")
            .requires_fs(true),
        ToolDef::new("find", "/usr/bin/find")
            .description("Search for files in a directory hierarchy")
            .requires_fs(true),
        ToolDef::new("stat", "/usr/bin/stat")
            .description("Display file status")
            .requires_fs(true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_message_serialization() {
        let msg = AgentMessage {
            id: "test".to_string(),
            message_type: MessageType::Observation,
            content: "test content".to_string(),
            metadata: serde_json::json!({}),
            timestamp: 0,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("observation"));

        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.message_type, MessageType::Observation);
    }

    #[test]
    fn test_tool_def_builder() {
        let tool = ToolDef::new("test", "/bin/echo")
            .description("A test tool")
            .requires_network(true)
            .max_timeout(60);

        assert_eq!(tool.name, "test");
        assert_eq!(tool.command, "/bin/echo");
        assert!(tool.requires_network);
        assert_eq!(tool.max_timeout, 60);
    }

    #[test]
    fn test_standard_tools() {
        let tools = standard_tools();
        assert!(!tools.is_empty());

        let read_tool = tools.iter().find(|t| t.name == "read_file");
        assert!(read_tool.is_some());
        assert!(read_tool.unwrap().requires_fs);
    }
}
