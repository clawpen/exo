use crate::protocol::{SessionId, GatewayMessage};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Connection handle for sending messages to a session
pub type SessionSender = mpsc::UnboundedSender<GatewayMessage>;

/// Information about an active session
#[derive(Debug, Clone)]
pub struct Session {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub agent_id: Option<String>,
    pub capabilities: Vec<String>,
    pub sender: SessionSender,
}

impl Session {
    pub fn new(sender: SessionSender) -> Self {
        let now = Utc::now();
        Self {
            id: SessionId::new(),
            created_at: now,
            last_activity: now,
            agent_id: None,
            capabilities: Vec::new(),
            sender,
        }
    }
    
    pub fn touch(&mut self) {
        self.last_activity = Utc::now();
    }
    
    pub fn is_stale(&self, timeout_secs: u64) -> bool {
        let elapsed = Utc::now().signed_duration_since(self.last_activity);
        elapsed.num_seconds() > timeout_secs as i64
    }
}

/// Manages all active agent sessions
#[derive(Debug, Clone)]
pub struct SessionManager {
    sessions: Arc<DashMap<SessionId, Session>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
        }
    }
    
    /// Create a new session
    pub fn create(&self, sender: SessionSender) -> SessionId {
        let session = Session::new(sender);
        let id = session.id.clone();
        self.sessions.insert(id.clone(), session);
        info!(session_id = %id, "Session created");
        id
    }
    
    /// Get a session by ID
    pub fn get(&self, id: &SessionId) -> Option<dashmap::mapref::one::Ref<SessionId, Session>> {
        self.sessions.get(id)
    }
    
    /// Get a mutable session reference
    pub fn get_mut(&self, id: &SessionId) -> Option<dashmap::mapref::one::RefMut<SessionId, Session>> {
        self.sessions.get_mut(id)
    }
    
    /// Update session info after Hello message
    pub fn update_hello(&self, id: &SessionId, agent_id: Option<String>, capabilities: Vec<String>) {
        if let Some(mut session) = self.sessions.get_mut(id) {
            session.agent_id = agent_id;
            session.capabilities = capabilities;
            session.touch();
            debug!(session_id = %id, "Session updated with agent info");
        }
    }
    
    /// Send a message to a specific session
    pub fn send_to(&self, id: &SessionId, message: GatewayMessage) -> Result<(), SessionError> {
        match self.sessions.get(id) {
            Some(session) => {
                session.sender.send(message).map_err(|_| SessionError::Disconnected)?;
                Ok(())
            }
            None => Err(SessionError::NotFound),
        }
    }
    
    /// Remove a session
    pub fn remove(&self, id: &SessionId) -> Option<Session> {
        let removed = self.sessions.remove(id).map(|(_, s)| s);
        if removed.is_some() {
            info!(session_id = %id, "Session removed");
        }
        removed
    }
    
    /// List all active sessions
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|entry| SessionInfo {
                id: entry.key().to_string(),
                agent_id: entry.value().agent_id.clone(),
                created_at: entry.value().created_at,
                last_activity: entry.value().last_activity,
                capabilities: entry.value().capabilities.clone(),
            })
            .collect()
    }
    
    /// Get session count
    pub fn count(&self) -> usize {
        self.sessions.len()
    }
    
    /// Clean up stale sessions
    pub fn cleanup_stale(&self, timeout_secs: u64) -> usize {
        let stale_ids: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|entry| entry.value().is_stale(timeout_secs))
            .map(|entry| entry.key().clone())
            .collect();
        
        for id in &stale_ids {
            self.remove(id);
            warn!(session_id = %id, "Removed stale session");
        }
        
        stale_ids.len()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub agent_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found")]
    NotFound,
    #[error("Session disconnected")]
    Disconnected,
}
