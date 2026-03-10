//! Agent memory backed by SQLite.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, Row, FromRow};
use std::path::Path;
use tracing::{debug, instrument};
use uuid::Uuid;

use crate::llm::Message;

/// Agent memory
pub struct AgentMemory {
    pool: SqlitePool,
    config: crate::config::MemoryConfig,
    session_id: String,
}

/// Conversation turn
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Conversation {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

impl AgentMemory {
    /// Create a new memory store
    pub async fn new(config: crate::config::MemoryConfig) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = config.db_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        
        let db_url = format!("sqlite:{}?mode=rwc", config.db_path.display());
        let pool = SqlitePool::connect(&db_url)
            .await
            .context("Failed to connect to memory database")?;
        
        // Run migrations
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE INDEX IF NOT EXISTS idx_session ON conversations(session_id);
        "#)
        .execute(&pool)
        .await
        .context("Failed to create memory tables")?;
        
        let session_id = Uuid::new_v4().to_string();
        
        debug!("Memory initialized: session={}", session_id);
        
        Ok(Self { pool, config, session_id })
    }
    
    /// Add a message to memory
    #[instrument(skip(self, content))]
    pub async fn add(&self, role: &str, content: &str) -> Result<i64> {
        let result = sqlx::query(r#"
            INSERT INTO conversations (session_id, role, content, created_at)
            VALUES (?, ?, ?, datetime('now'))
            RETURNING id
        "#)
        .bind(&self.session_id)
        .bind(role)
        .bind(content)
        .fetch_one(&self.pool)
        .await
        .context("Failed to add message to memory")?;
        
        let id: i64 = result.get("id");
        debug!("Added message to memory: id={}, role={}", id, role);
        Ok(id)
    }
    
    /// Get recent messages
    pub async fn get_recent(&self, limit: usize) -> Result<Vec<Conversation>> {
        let limit = limit.min(self.config.max_turns.unwrap_or(50) * 2);
        
        let rows: Vec<Conversation> = sqlx::query_as(r#"
            SELECT id, session_id, role, content, created_at
            FROM conversations
            WHERE session_id = ?
            ORDER BY id DESC
            LIMIT ?
        "#)
        .bind(&self.session_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .context("Failed to get recent messages")?;
        
        // Reverse to get chronological order
        let mut rows = rows;
        rows.reverse();
        Ok(rows)
    }
    
    /// Get messages as LLM format
    pub async fn get_context(&self, limit: usize) -> Result<Vec<Message>> {
        let conversations = self.get_recent(limit).await?;
        
        Ok(conversations.into_iter().map(|c| Message {
            role: match c.role.as_str() {
                "system" => crate::llm::Role::System,
                "user" => crate::llm::Role::User,
                "assistant" => crate::llm::Role::Assistant,
                "tool" => crate::llm::Role::Tool,
                _ => crate::llm::Role::User,
            },
            content: c.content,
            tool_calls: None,
            tool_call_id: None,
        }).collect())
    }
    
    /// Clear current session
    pub async fn clear(&self) -> Result<()> {
        sqlx::query("DELETE FROM conversations WHERE session_id = ?")
            .bind(&self.session_id)
            .execute(&self.pool)
            .await
            .context("Failed to clear memory")?;
        
        debug!("Cleared memory for session {}", self.session_id);
        Ok(())
    }
    
    /// Start a new session
    pub fn new_session(&mut self) {
        self.session_id = Uuid::new_v4().to_string();
        debug!("Started new session: {}", self.session_id);
    }
    
    /// Get current session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    
    /// Export memory to JSON
    pub async fn export(&self) -> Result<Vec<Conversation>> {
        self.get_recent(1000).await
    }
}
