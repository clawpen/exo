//! # Exo Gateway
//! 
//! Agent-first gateway server for the Exo container runtime.
//! 
//! ## Features
//! 
//! - **WebSocket-based agent communication** - No HTTP overhead for tool calls
//! - **Session management** - Track and manage active agent sessions
//! - **Skill registry** - Containerized and WASM skill execution
//! - **Cron scheduling** - Time-based job execution
//! - **REST API** - External integrations and monitoring

pub mod cron;
pub mod protocol;
pub mod server;
pub mod session;
pub mod skill;

pub use cron::{CronScheduler, CreateJobRequest, ScheduledJob, CronError};
pub use protocol::{GatewayMessage, ErrorCode, ToolResult, SessionId, RequestId, PROTOCOL_VERSION};
pub use server::{GatewayServer, AppState, ServerError};
pub use session::{SessionManager, Session, SessionInfo, SessionError};
pub use skill::{SkillRegistry, SkillManifest, SkillRuntime, ToolDef, Skill, SkillError};

use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

/// Gateway configuration
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub bind_addr: SocketAddr,
    pub skills_dir: Option<std::path::PathBuf>,
    pub session_timeout_secs: u64,
    pub enable_cron: bool,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".parse().unwrap(),
            skills_dir: None,
            session_timeout_secs: 300, // 5 minutes
            enable_cron: true,
        }
    }
}

/// Main gateway application
pub struct Gateway {
    config: GatewayConfig,
    session_manager: Arc<SessionManager>,
    skill_registry: Arc<SkillRegistry>,
    cron_scheduler: Option<Arc<CronScheduler>>,
}

impl Gateway {
    /// Create a new gateway instance
    pub async fn new(config: GatewayConfig) -> Result<Self, GatewayError> {
        let session_manager = Arc::new(SessionManager::new());
        
        let skill_registry = if let Some(ref dir) = config.skills_dir {
            let registry = SkillRegistry::with_skills_dir(dir);
            let count = registry.load_from_dir(dir).await
                .map_err(|e| GatewayError::SkillRegistry(e.to_string()))?;
            info!(count, "Loaded skills from directory");
            Arc::new(registry)
        } else {
            Arc::new(SkillRegistry::new())
        };
        
        let cron_scheduler = if config.enable_cron {
            let scheduler = CronScheduler::new(session_manager.clone()).await
                .map_err(|e| GatewayError::Scheduler(e.to_string()))?;
            Some(Arc::new(scheduler))
        } else {
            None
        };
        
        Ok(Self {
            config,
            session_manager,
            skill_registry,
            cron_scheduler,
        })
    }
    
    /// Run the gateway server
    pub async fn run(self) -> Result<(), GatewayError> {
        // Start cron scheduler
        if let Some(ref scheduler) = self.cron_scheduler {
            scheduler.start().await
                .map_err(|e| GatewayError::Scheduler(e.to_string()))?;
        }
        
        // Start session cleanup task
        let session_manager = self.session_manager.clone();
        let timeout = self.config.session_timeout_secs;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let cleaned = session_manager.cleanup_stale(timeout);
                if cleaned > 0 {
                    info!(count = cleaned, "Cleaned up stale sessions");
                }
            }
        });
        
        // Create and run server
        let server = GatewayServer::new(
            self.config.bind_addr,
            self.session_manager,
            self.skill_registry,
            self.cron_scheduler.unwrap_or_else(|| {
                // Create dummy scheduler if disabled
                Arc::new(tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        CronScheduler::new(Arc::new(SessionManager::new())).await.unwrap()
                    })
                }))
            }),
        );
        
        info!("Gateway ready at {}", self.config.bind_addr);
        
        server.run().await
            .map_err(|e| GatewayError::Server(e.to_string()))?;
        
        Ok(())
    }
    
    /// Get session manager
    pub fn session_manager(&self) -> Arc<SessionManager> {
        self.session_manager.clone()
    }
    
    /// Get skill registry
    pub fn skill_registry(&self) -> Arc<SkillRegistry> {
        self.skill_registry.clone()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("Skill registry error: {0}")]
    SkillRegistry(String),
    #[error("Scheduler error: {0}")]
    Scheduler(String),
    #[error("Server error: {0}")]
    Server(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_session_id_parsing() {
        let id = SessionId::new();
        let str_repr = id.to_string();
        let parsed = SessionId::try_from(str_repr.as_str()).unwrap();
        assert_eq!(str_repr, parsed.to_string());
    }
    
    #[tokio::test]
    async fn test_skill_registry() {
        let registry = SkillRegistry::new();
        
        let manifest = SkillManifest {
            name: "test-skill".to_string(),
            version: "1.0.0".to_string(),
            description: "Test skill".to_string(),
            author: Some("Test".to_string()),
            runtime: SkillRuntime::Builtin,
            tools: vec![ToolDef {
                name: "test-tool".to_string(),
                description: "A test tool".to_string(),
                parameters: serde_json::json!({}),
                returns: None,
                timeout_ms: None,
            }],
            config: None,
        };
        
        registry.register(manifest, None).await.unwrap();
        
        let skills = registry.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
    }
}
