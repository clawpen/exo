use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};

/// Protocol version for compatibility
pub const PROTOCOL_VERSION: &str = "1.0.0";

/// Core message types for agent-gateway communication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayMessage {
    // Connection lifecycle
    Hello {
        version: String,
        agent_id: Option<String>,
        capabilities: Vec<String>,
    },
    Welcome {
        session_id: String,
        server_version: String,
    },
    
    // Tool/skill invocation
    ToolRequest {
        request_id: String,
        skill: String,
        tool: String,
        args: serde_json::Value,
        timeout_ms: Option<u64>,
    },
    ToolResponse {
        request_id: String,
        #[serde(flatten)]
        result: ToolResult,
    },
    
    // Streaming/observations
    Observation {
        request_id: String,
        content: String,
        done: bool,
    },
    
    // Session management
    SessionInfo {
        session_id: String,
        created_at: DateTime<Utc>,
        active_tools: Vec<String>,
    },
    
    // Errors
    Error {
        request_id: Option<String>,
        code: ErrorCode,
        message: String,
    },
    
    // Heartbeat
    Ping,
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResult {
    Success {
        output: serde_json::Value,
        execution_time_ms: u64,
    },
    Error {
        code: String,
        message: String,
    },
    Timeout {
        timeout_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidMessage,
    UnknownSkill,
    UnknownTool,
    ToolExecutionFailed,
    Timeout,
    Unauthorized,
    RateLimited,
    InternalError,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCode::InvalidMessage => write!(f, "invalid_message"),
            ErrorCode::UnknownSkill => write!(f, "unknown_skill"),
            ErrorCode::UnknownTool => write!(f, "unknown_tool"),
            ErrorCode::ToolExecutionFailed => write!(f, "tool_execution_failed"),
            ErrorCode::Timeout => write!(f, "timeout"),
            ErrorCode::Unauthorized => write!(f, "unauthorized"),
            ErrorCode::RateLimited => write!(f, "rate_limited"),
            ErrorCode::InternalError => write!(f, "internal_error"),
        }
    }
}

/// Session identification
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> Self {
        id.to_string()
    }
}

impl TryFrom<&str> for SessionId {
    type Error = uuid::Error;
    
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(SessionId(Uuid::parse_str(value)?))
    }
}

/// Request ID for tracking tool calls
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<RequestId> for String {
    fn from(id: RequestId) -> Self {
        id.to_string()
    }
}
