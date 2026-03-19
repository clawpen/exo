use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{info, warn};
use uuid::Uuid;

use crate::protocol::GatewayMessage;
use crate::session::SessionManager;

/// A scheduled job definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub cron: String,
    pub skill: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub target_session: Option<String>, // None = broadcast to all sessions
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub run_count: u64,
}

/// Request to create a scheduled job
#[derive(Debug, Clone, Deserialize)]
pub struct CreateJobRequest {
    pub name: String,
    pub description: Option<String>,
    pub cron: String,
    pub skill: String,
    pub tool: String,
    pub args: serde_json::Value,
    pub target_session: Option<String>,
}

/// Manages scheduled jobs
pub struct CronScheduler {
    scheduler: JobScheduler,
    jobs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, ScheduledJob>>>,
    session_manager: Arc<SessionManager>,
    job_tx: mpsc::Sender<JobEvent>,
    job_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<JobEvent>>>,
}

impl std::fmt::Debug for CronScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CronScheduler")
            .field("jobs", &self.jobs)
            .field("session_manager", &self.session_manager)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum JobEvent {
    Execute { job_id: String },
    Stop,
}

impl CronScheduler {
    pub async fn new(session_manager: Arc<SessionManager>) -> Result<Self, CronError> {
        let scheduler = JobScheduler::new().await
            .map_err(|e| CronError::Scheduler(e.to_string()))?;
        
        let (job_tx, job_rx) = mpsc::channel(100);
        
        Ok(Self {
            scheduler,
            jobs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            session_manager,
            job_tx,
            job_rx: Arc::new(tokio::sync::Mutex::new(job_rx)),
        })
    }
    
    /// Start the scheduler
    pub async fn start(&self) -> Result<(), CronError> {
        self.scheduler.start().await
            .map_err(|e| CronError::Scheduler(e.to_string()))?;
        
        info!("Cron scheduler started");
        
        // Start event processor
        let jobs = self.jobs.clone();
        let session_manager = self.session_manager.clone();
        let job_rx = self.job_rx.clone();
        
        tokio::spawn(async move {
            let mut rx = job_rx.lock().await;
            while let Some(event) = rx.recv().await {
                match event {
                    JobEvent::Execute { job_id } => {
                        Self::execute_job(&jobs, &session_manager, &job_id).await;
                    }
                    JobEvent::Stop => break,
                }
            }
        });
        
        Ok(())
    }
    
    /// Create and schedule a new job
    pub async fn create_job(&self,
        request: CreateJobRequest,
    ) -> Result<ScheduledJob, CronError> {
        // Validate cron expression
        let _schedule = Schedule::from_str(&request.cron)
            .map_err(|e| CronError::InvalidCron(e.to_string()))?;
        
        let job_id = Uuid::new_v4().to_string();
        let job = ScheduledJob {
            id: job_id.clone(),
            name: request.name,
            description: request.description,
            cron: request.cron.clone(),
            skill: request.skill,
            tool: request.tool,
            args: request.args,
            target_session: request.target_session,
            enabled: true,
            created_at: Utc::now(),
            last_run: None,
            run_count: 0,
        };
        
        // Store job
        self.jobs.write().await.insert(job_id.clone(), job.clone());
        
        // Create scheduler job
        let job_tx = self.job_tx.clone();
        let id = job_id.clone();
        let cron_expr = request.cron.clone();
        
        let sched_job = Job::new_async(cron_expr.as_str(), move |_uuid, _l| {
            let tx = job_tx.clone();
            let job_id = id.clone();
            Box::pin(async move {
                let _ = tx.send(JobEvent::Execute { job_id }).await;
            })
        }).map_err(|e| CronError::Scheduler(e.to_string()))?;
        
        self.scheduler.add(sched_job).await
            .map_err(|e| CronError::Scheduler(e.to_string()))?;
        
        info!(job_id = %job_id, name = %job.name, cron = %request.cron, "Job scheduled");
        Ok(job)
    }
    
    /// Execute a job (internal)
    async fn execute_job(
        jobs: &Arc<tokio::sync::RwLock<std::collections::HashMap<String, ScheduledJob>>>,
        session_manager: &Arc<SessionManager>,
        job_id: &str,
    ) {
        let job = match jobs.read().await.get(job_id) {
            Some(j) if j.enabled => j.clone(),
            _ => return,
        };
        
        info!(job_id = %job_id, name = %job.name, "Executing scheduled job");
        
        // Build tool request message
        let request_id = Uuid::new_v4().to_string();
        let message = GatewayMessage::ToolRequest {
            request_id,
            skill: job.skill,
            tool: job.tool,
            args: job.args,
            timeout_ms: Some(300_000), // 5 min default for cron jobs
        };
        
        // Send to target session or broadcast
        match &job.target_session {
            Some(session_id) => {
                match crate::protocol::SessionId::try_from(session_id.as_str()) {
                    Ok(id) => {
                        if let Err(e) = session_manager.send_to(&id, message) {
                            warn!(job_id = %job_id, error = %e, "Failed to send to target session");
                        }
                    }
                    Err(_) => {
                        warn!(job_id = %job_id, session_id = %session_id, "Invalid session ID");
                    }
                }
            }
            None => {
                // Broadcast to all sessions (simplified)
                for session in session_manager.list() {
                    if let Ok(id) = crate::protocol::SessionId::try_from(session.id.as_str()) {
                        let _ = session_manager.send_to(&id, message.clone());
                    }
                }
            }
        }
        
        // Update job stats
        if let Some(j) = jobs.write().await.get_mut(job_id) {
            j.last_run = Some(Utc::now());
            j.run_count += 1;
        }
    }
    
    /// List all jobs
    pub async fn list_jobs(&self) -> Vec<ScheduledJob> {
        self.jobs.read().await.values().cloned().collect()
    }
    
    /// Get a specific job
    pub async fn get_job(&self, job_id: &str) -> Option<ScheduledJob> {
        self.jobs.read().await.get(job_id).cloned()
    }
    
    /// Disable/enable a job
    pub async fn toggle_job(&self, job_id: &str, enabled: bool) -> Result<(), CronError> {
        if let Some(job) = self.jobs.write().await.get_mut(job_id) {
            job.enabled = enabled;
            info!(job_id = %job_id, enabled, "Job toggled");
            Ok(())
        } else {
            Err(CronError::NotFound(job_id.to_string()))
        }
    }
    
    /// Remove a job
    pub async fn remove_job(&self, job_id: &str) -> Result<(), CronError> {
        if self.jobs.write().await.remove(job_id).is_some() {
            info!(job_id = %job_id, "Job removed");
            Ok(())
        } else {
            Err(CronError::NotFound(job_id.to_string()))
        }
    }
    
    /// Stop the scheduler
    pub async fn shutdown(&mut self) -> Result<(), CronError> {
        let _ = self.job_tx.send(JobEvent::Stop).await;
        self.scheduler.shutdown().await
            .map_err(|e| CronError::Scheduler(e.to_string()))?;
        info!("Cron scheduler stopped");
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("Scheduler error: {0}")]
    Scheduler(String),
    #[error("Invalid cron expression: {0}")]
    InvalidCron(String),
    #[error("Job not found: {0}")]
    NotFound(String),
}
