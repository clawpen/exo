//! Tool system for agent capabilities

use crate::protocol::{ToolDefinition, ToolPermission};
use crate::{Error, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Base trait for all tools
pub trait Tool: Send + Sync {
    /// Execute the tool with given arguments
    fn execute(&self, args: Value) -> Result<Value>;

    /// Get tool definition for agent
    fn definition(&self) -> ToolDefinition;
}

/// Registry of available tools
#[derive(Clone)]
pub struct ToolRegistry {
    tools: Arc<Mutex<HashMap<String, Box<dyn Tool>>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register(&self, tool: Box<dyn Tool>) {
        let name = tool.definition().name.clone();
        self.tools.lock().unwrap().insert(name, tool);
    }

    pub fn get(&self, _name: &str) -> Option<Box<dyn Tool>> {
        // Note: This can't actually return the tool from HashMap due to trait object limitations
        // In real implementation, we'd use execute() directly on the registry
        None
    }

    pub fn execute(&self, name: &str, args: Value) -> Result<Value> {
        let tools = self.tools.lock().unwrap();
        let tool = tools
            .get(name)
            .ok_or_else(|| Error::ToolNotFound(name.to_string()))?;
        tool.execute(args)
    }

    pub fn list_definitions(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.lock().unwrap();
        tools.values().map(|t| t.definition()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// DateTime tool - safe, always available
pub struct DateTimeTool;

impl Tool for DateTimeTool {
    fn execute(&self, _args: Value) -> Result<Value> {
        let now = chrono::Local::now();
        let offset = now.offset();
        Ok(serde_json::json!({
            "datetime": now.to_rfc3339(),
            "unix_timestamp": now.timestamp(),
            "timezone": offset.to_string()
        }))
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "get_datetime".to_string(),
            description: "Get current date and time".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            requires_permission: ToolPermission::None,
        }
    }
}
