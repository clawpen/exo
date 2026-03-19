use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

use crate::{
    cron::{CreateJobRequest, CronScheduler},
    protocol::{ErrorCode, GatewayMessage, SessionId},
    session::SessionManager,
    skill::SkillRegistry,
};

/// Shared application state
#[derive(Debug, Clone)]
pub struct AppState {
    pub session_manager: Arc<SessionManager>,
    pub skill_registry: Arc<SkillRegistry>,
    pub cron_scheduler: Arc<CronScheduler>,
}

/// Gateway server
pub struct GatewayServer {
    state: AppState,
    addr: SocketAddr,
}

impl GatewayServer {
    pub fn new(
        addr: SocketAddr,
        session_manager: Arc<SessionManager>,
        skill_registry: Arc<SkillRegistry>,
        cron_scheduler: Arc<CronScheduler>,
    ) -> Self {
        Self {
            state: AppState {
                session_manager,
                skill_registry,
                cron_scheduler,
            },
            addr,
        }
    }
    
    pub async fn run(self) -> Result<(), ServerError> {
        let app = Router::new()
            // WebSocket endpoint for agents
            .route("/ws", get(ws_handler))
            .route("/ws/:session_id", get(ws_handler_with_id))
            // REST API
            .route("/health", get(health_check))
            .route("/sessions", get(list_sessions))
            .route("/skills", get(list_skills))
            .route("/tools", get(list_tools))
            .route("/invoke/:skill/:tool", post(invoke_tool))
            .route("/cron/jobs", get(list_jobs).post(create_job))
            .route("/cron/jobs/:job_id", get(get_job).delete(delete_job))
            .route("/cron/jobs/:job_id/toggle", post(toggle_job))
            .layer(CorsLayer::permissive())
            .layer(TraceLayer::new_for_http())
            .with_state(self.state.clone());
        
        info!(addr = %self.addr, "Gateway server starting");
        
        let listener = tokio::net::TcpListener::bind(self.addr).await
            .map_err(|e| ServerError::Bind(e.to_string()))?;
        
        axum::serve(listener, app).await
            .map_err(|e| ServerError::Server(e.to_string()))?;
        
        Ok(())
    }
}

/// WebSocket handler for new connections
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, None))
}

/// WebSocket handler with specific session ID (reconnection)
async fn ws_handler_with_id(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, Some(session_id)))
}

/// Main WebSocket connection handler
async fn handle_socket(socket: WebSocket, state: AppState, reconnect_id: Option<String>) {
    let (mut sender, mut receiver) = socket.split();
    
    // Create channel for sending messages to this session
    let (tx, mut rx) = mpsc::unbounded_channel::<GatewayMessage>();
    
    // Create or restore session
    let session_id = if let Some(id_str) = reconnect_id {
        match SessionId::try_from(id_str.as_str()) {
            Ok(id) => {
                // Check if session exists, if not create new
                if state.session_manager.get(&id).is_some() {
                    // TODO: Update session sender for reconnection
                    id
                } else {
                    state.session_manager.create(tx)
                }
            }
            Err(_) => state.session_manager.create(tx),
        }
    } else {
        state.session_manager.create(tx)
    };
    
    info!(session_id = %session_id, "Agent connected");
    
    // Send welcome message
    let welcome = GatewayMessage::Welcome {
        session_id: session_id.to_string(),
        server_version: crate::protocol::PROTOCOL_VERSION.to_string(),
    };
    
    if let Err(e) = sender.send(Message::Text(serde_json::to_string(&welcome).unwrap())).await {
        warn!(session_id = %session_id, error = %e, "Failed to send welcome");
        return;
    }
    
    // Spawn task to handle outgoing messages
    let session_id_clone = session_id.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let text = match serde_json::to_string(&msg) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Failed to serialize message");
                    continue;
                }
            };
            
            if let Err(e) = sender.send(Message::Text(text)).await {
                warn!(session_id = %session_id_clone, error = %e, "Failed to send message");
                break;
            }
        }
    });
    
    // Handle incoming messages
    let session_id_clone = session_id.clone();
    let state_clone = state.clone();
    
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                match serde_json::from_str::<GatewayMessage>(&text) {
                    Ok(gateway_msg) => {
                        if let Err(e) = handle_message(
                            &session_id_clone,
                            gateway_msg,
                            &state_clone,
                        ).await {
                            warn!(session_id = %session_id_clone, error = %e, "Error handling message");
                        }
                    }
                    Err(e) => {
                        warn!(session_id = %session_id_clone, error = %e, "Invalid message format");
                        let _ = state_clone.session_manager.send_to(
                            &session_id_clone,
                            GatewayMessage::Error {
                                request_id: None,
                                code: ErrorCode::InvalidMessage,
                                message: format!("Invalid message: {}", e),
                            },
                        );
                    }
                }
            }
            Message::Close(_) => {
                info!(session_id = %session_id_clone, "Agent disconnected");
                break;
            }
            _ => {} // Ignore other message types
        }
    }
    
    // Clean up
    send_task.abort();
    state.session_manager.remove(&session_id);
    info!(session_id = %session_id, "Session closed");
}

/// Handle a single gateway message
async fn handle_message(
    session_id: &SessionId,
    msg: GatewayMessage,
    state: &AppState,
) -> Result<(), MessageError> {
    match msg {
        GatewayMessage::Hello { version, agent_id, capabilities } => {
            debug!(session_id = %session_id, version = %version, "Agent hello");
            state.session_manager.update_hello(session_id, agent_id, capabilities);
            
            // Send session info back
            let info = GatewayMessage::SessionInfo {
                session_id: session_id.to_string(),
                created_at: chrono::Utc::now(),
                active_tools: state.skill_registry.list_tools().await.iter().map(|t| t.name.clone()).collect(),
            };
            state.session_manager.send_to(session_id, info)
                .map_err(|_| MessageError::SessionClosed)?;
        }
        
        GatewayMessage::ToolRequest { request_id, skill, tool, args, timeout_ms } => {
            debug!(request_id = %request_id, skill = %skill, tool = %tool, "Tool request");
            
            // Check if skill exists
            if state.skill_registry.get(&skill).await.is_none() {
                state.session_manager.send_to(session_id, GatewayMessage::ToolResponse {
                    request_id,
                    result: crate::protocol::ToolResult::Error {
                        code: ErrorCode::UnknownSkill.to_string(),
                        message: format!("Skill '{}' not found", skill),
                    },
                }).map_err(|_| MessageError::SessionClosed)?;
                return Ok(());
            }
            
            // For now, echo back a success (actual execution would go through exo-runtime)
            // TODO: Integrate with exo-runtime for actual tool execution
            state.session_manager.send_to(session_id, GatewayMessage::ToolResponse {
                request_id,
                result: crate::protocol::ToolResult::Success {
                    output: serde_json::json!({
                        "message": format!("Tool {}:{} would execute with args: {:?}", skill, tool, args),
                        "note": "Integration with exo-runtime pending"
                    }),
                    execution_time_ms: 0,
                },
            }).map_err(|_| MessageError::SessionClosed)?;
        }
        
        GatewayMessage::Ping => {
            state.session_manager.send_to(session_id, GatewayMessage::Pong)
                .map_err(|_| MessageError::SessionClosed)?;
        }
        
        _ => {
            debug!("Unhandled message type");
        }
    }
    
    // Update activity
    if let Some(mut session) = state.session_manager.get_mut(session_id) {
        (*session).touch();
    }
    
    Ok(())
}

// REST API handlers

async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": crate::protocol::PROTOCOL_VERSION,
        "sessions": state.session_manager.count(),
    }))
}

async fn list_sessions(State(state): State<AppState>) -> impl IntoResponse {
    let sessions: Vec<_> = state.session_manager.list().into_iter().map(|s| {
        serde_json::json!({
            "id": s.id,
            "agent_id": s.agent_id,
            "created_at": s.created_at,
            "last_activity": s.last_activity,
            "capabilities": s.capabilities,
        })
    }).collect();
    
    Json(sessions)
}

async fn list_skills(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.skill_registry.list_skills().await)
}

async fn list_tools(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.skill_registry.list_tools().await)
}

async fn invoke_tool(
    State(_state): State<AppState>,
    Path((skill, tool)): Path<(String, String)>,
    Json(args): Json<serde_json::Value>,
) -> Result<impl IntoResponse, StatusCode> {
    // TODO: Actual tool execution via exo-runtime
    Ok(Json(serde_json::json!({
        "skill": skill,
        "tool": tool,
        "args": args,
        "status": "not_implemented",
        "note": "Direct HTTP tool invocation pending exo-runtime integration"
    })))
}

async fn list_jobs(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.cron_scheduler.list_jobs().await)
}

async fn create_job(
    State(state): State<AppState>,
    Json(request): Json<CreateJobRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.cron_scheduler.create_job(request).await {
        Ok(job) => Ok((StatusCode::CREATED, Json(job))),
        Err(e) => {
            error!(error = %e, "Failed to create job");
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

async fn get_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.cron_scheduler.get_job(&job_id).await {
        Some(job) => Ok(Json(job)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn delete_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.cron_scheduler.remove_job(&job_id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

async fn toggle_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, StatusCode> {
    let enabled = body.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    match state.cron_scheduler.toggle_job(&job_id, enabled).await {
        Ok(_) => Ok(Json(serde_json::json!({"enabled": enabled}))),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

#[derive(Debug, thiserror::Error)]
enum MessageError {
    #[error("Session closed")]
    SessionClosed,
}

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("Failed to bind: {0}")]
    Bind(String),
    #[error("Server error: {0}")]
    Server(String),
}
