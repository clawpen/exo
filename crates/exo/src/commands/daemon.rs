//! Daemon mode for exo - runs a persistent server for faster command execution

use anyhow::Result;
use serde::{Deserialize, Serialize};
#[cfg(all(unix, not(target_os = "macos")))]
use std::collections::HashMap;
#[cfg(all(unix, not(target_os = "macos")))]
use std::io::{BufRead, BufReader, Write};
#[cfg(all(unix, not(target_os = "macos")))]
use std::path::PathBuf;
#[cfg(all(unix, not(target_os = "macos")))]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(all(unix, not(target_os = "macos")))]
use std::thread;
#[cfg(all(unix, not(target_os = "macos")))]
use std::time::Duration;

/// Tokio runtime handle captured at daemon startup so worker threads (which
/// run outside the tokio context) can `block_on` async work like image pulls.
#[cfg(all(unix, not(target_os = "macos")))]
static RUNTIME_HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();

#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::fs::PermissionsExt;
#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::net::{UnixListener, UnixStream};

#[cfg(all(unix, not(target_os = "macos")))]
use exo_runtime::{
    Container, ContainerJson, ContainerListJson, ContainerManager, ContainerMetadata, EventLog,
    EventType, ReconcileOptions, Reconciler,
};
#[cfg(all(unix, not(target_os = "macos")))]
use tokio::sync::Semaphore;

/// Default cap on concurrent `execute_run` operations. Tunable via
/// `EXO_MAX_CONCURRENT_STARTS`. Picked to keep WSL2 from thrashing under
/// fan-out at N=1000 — measured empirically; revisit when the daemon
/// actually pulls images itself (see project_v2_image_pull_dedup memory).
#[cfg(all(unix, not(target_os = "macos")))]
const DEFAULT_MAX_CONCURRENT_STARTS: usize = 32;

/// Default cap on in-flight client connections. Tunable via
/// `EXO_MAX_CONCURRENT_CONNS`. Each connection currently consumes a
/// thread, so this guards against thread/stack OOM under thundering herd.
#[cfg(all(unix, not(target_os = "macos")))]
const DEFAULT_MAX_CONCURRENT_CONNS: usize = 256;

const SOCKET_PATH: &str = "/tmp/exo-daemon.sock";
const PID_FILE: &str = "/tmp/exo-daemon.pid";

/// Daemon configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub socket_path: String,
    pub timeout_ms: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: SOCKET_PATH.to_string(),
            timeout_ms: 30000, // 30 seconds
        }
    }
}

pub struct DaemonArgs {
    pub socket_path: Option<String>,
    pub timeout: Option<u64>,
    pub foreground: bool,
    pub stop: bool,
}

pub struct DaemonStatusArgs {
    pub json: bool,
}

/// Container specification for daemon communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub workdir: String,
    pub env: Vec<String>,
    pub command: Vec<String>,
    pub mounts: Vec<MountSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

/// Start the daemon
#[cfg(all(unix, not(target_os = "macos")))]
pub fn start(args: DaemonArgs) -> Result<()> {
    let config = DaemonConfig {
        socket_path: args.socket_path.unwrap_or_else(|| SOCKET_PATH.to_string()),
        timeout_ms: args.timeout.unwrap_or(30000),
    };

    // Check if daemon is already running
    if is_daemon_running(&config.socket_path) {
        eprintln!("Exo daemon is already running");
        eprintln!("Use 'exo daemon stop' to stop it first");
        return Ok(());
    }

    // Clean up any stale socket
    let _ = std::fs::remove_file(&config.socket_path);

    // Write PID file
    if let Err(e) = std::fs::write(PID_FILE, format!("{}\n", std::process::id())) {
        eprintln!("Warning: Could not write PID file: {}", e);
    }

    println!("Starting exo daemon on socket: {}", config.socket_path);

    if args.foreground {
        run_daemon(config)?;
    } else {
        // Fork to background (simplified - just spawn a new process)
        println!("Running in background...");
        // In a real implementation we'd use fork/daemon
        // For now, just run in foreground with a note
        run_daemon(config)?;
    }

    Ok(())
}

/// Start the daemon (Windows - starts WSL2 daemon)
#[cfg(windows)]
pub fn start(args: DaemonArgs) -> Result<()> {
    use exo_wsl::WslCommand;
    use exo_wsl::WslConfig;

    println!("Starting exo daemon in WSL2...");

    let wsl_cmd = WslCommand::new(WslConfig::default());

    // Check if daemon is already running
    let check_result =
        wsl_cmd.exec("test -S /tmp/exo-daemon.sock && echo 'RUNNING' || echo 'NOT_RUNNING'")?;

    if check_result.stdout.contains("RUNNING") {
        println!("Exo daemon is already running in WSL2");
        println!("Use 'exo daemon stop' to stop it first");
        return Ok(());
    }

    // Start the daemon in background
    let start_cmd =
        "setsid exo-runtime daemon --foreground > /tmp/exo-daemon.log 2>&1 < /dev/null & echo $!";
    let result = wsl_cmd.exec(start_cmd)?;

    if result.exit_code == 0 {
        println!("Exo daemon started in WSL2 (PID: {})", result.stdout.trim());
        println!("Socket: /tmp/exo-daemon.sock");

        // Wait and verify
        std::thread::sleep(std::time::Duration::from_millis(500));

        let verify_result =
            wsl_cmd.exec("test -S /tmp/exo-daemon.sock && echo 'RUNNING' || echo 'NOT_RUNNING'")?;
        if verify_result.stdout.contains("RUNNING") {
            println!("Daemon is running and ready");
        } else {
            println!("Warning: Daemon may not have started properly");
            let _ = wsl_cmd.exec("cat /tmp/exo-daemon.log 2>/dev/null | tail -20");
        }
    } else {
        return Err(exo_runtime::ExoError::BackendUnavailable(format!(
            "failed to start daemon in WSL2: {}",
            result.stderr
        ))
        .into());
    }

    Ok(())
}

/// Start the daemon (macOS native backend).
#[cfg(target_os = "macos")]
pub fn start(_args: DaemonArgs) -> Result<()> {
    println!("The native macOS backend runs directly and does not require an Exo daemon.");
    println!("Use `exo run --detach ...` for background processes.");
    Ok(())
}

/// Stop the daemon
#[cfg(all(unix, not(target_os = "macos")))]
pub fn stop() -> Result<()> {
    // Read PID file
    if let Ok(pid_str) = std::fs::read_to_string(PID_FILE) {
        let pid: u32 = pid_str.trim().parse()?;
        println!("Stopping exo daemon (PID: {})...", pid);

        // Send SIGTERM
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }

        // Wait for process to exit
        thread::sleep(Duration::from_secs(2));

        // Check if still running
        if is_process_running(pid) {
            println!("Daemon did not stop gracefully, forcing...");
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
            thread::sleep(Duration::from_millis(500));
        }

        // Clean up socket and PID file
        let _ = std::fs::remove_file(SOCKET_PATH);
        let _ = std::fs::remove_file(PID_FILE);

        println!("Daemon stopped");
    } else {
        eprintln!("Daemon is not running");
    }

    Ok(())
}

/// Stop the daemon (Windows - stops WSL2 daemon)
#[cfg(windows)]
pub fn stop() -> Result<()> {
    use exo_wsl::WslCommand;
    use exo_wsl::WslConfig;

    println!("Stopping exo daemon in WSL2...");

    let wsl_cmd = WslCommand::new(WslConfig::default());

    // Stop the daemon
    let result = wsl_cmd
        .exec("exo-runtime daemon shutdown 2>/dev/null || pkill -f 'exo.*daemon' || true")?;

    println!("Daemon stopped");

    // Clean up socket
    let _ = wsl_cmd.exec("rm -f /tmp/exo-daemon.sock /tmp/exo-daemon.pid");

    Ok(())
}

/// Stop the daemon (macOS native backend).
#[cfg(target_os = "macos")]
pub fn stop() -> Result<()> {
    println!("No Exo daemon is running for the native macOS backend.");
    Ok(())
}

/// Check daemon status
#[cfg(all(unix, not(target_os = "macos")))]
pub fn status(args: DaemonStatusArgs) -> Result<()> {
    if is_daemon_running(SOCKET_PATH) {
        if args.json {
            println!(r#"{{"running": true}}"#);
        } else {
            println!("Exo daemon is running");
            println!("Socket: {}", SOCKET_PATH);
        }
        Ok(())
    } else {
        if args.json {
            println!(r#"{{"running": false}}"#);
        } else {
            println!("Exo daemon is not running");
        }
        Ok(())
    }
}

/// Check daemon status (Windows - checks WSL2 daemon)
#[cfg(windows)]
pub fn status(args: DaemonStatusArgs) -> Result<()> {
    use exo_wsl::WslCommand;
    use exo_wsl::WslConfig;

    let wsl_cmd = WslCommand::new(WslConfig::default());

    // Check if socket exists
    let result =
        wsl_cmd.exec("test -S /tmp/exo-daemon.sock && echo 'RUNNING' || echo 'NOT_RUNNING'")?;

    let running = result.stdout.contains("RUNNING");

    if args.json {
        let status_json = if running {
            r#"{"running": true, "socket": "/tmp/exo-daemon.sock"}"#
        } else {
            r#"{"running": false}"#
        };
        println!("{}", status_json);
    } else {
        if running {
            println!("Exo daemon is running in WSL2");
            println!("Socket: /tmp/exo-daemon.sock");
        } else {
            println!("Exo daemon is not running in WSL2");
            println!("Start with: exo daemon");
        }
    }

    Ok(())
}

/// Check daemon status (macOS native backend).
#[cfg(target_os = "macos")]
pub fn status(args: DaemonStatusArgs) -> Result<()> {
    if args.json {
        println!(r#"{{"running": false, "backend": "native-macos", "required": false}}"#);
    } else {
        println!("Exo daemon is not required on the native macOS backend");
        println!("Backend: native-macos");
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_daemon_running(socket_path: &str) -> bool {
    std::path::Path::new(socket_path).exists()
}

#[cfg(target_os = "linux")]
fn is_process_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(target_os = "linux"))]
fn is_process_running(_pid: u32) -> bool {
    false
}

/// Run the daemon server
#[cfg(all(unix, not(target_os = "macos")))]
fn run_daemon(config: DaemonConfig) -> Result<()> {
    // Open the event log (best-effort; daemon still runs without it).
    let events = match EventLog::open_default() {
        Ok(log) => Some(log),
        Err(e) => {
            tracing::warn!(
                "event log unavailable, lifecycle events will not be persisted: {}",
                e
            );
            None
        }
    };

    // Build the reconciler.
    let manager = Arc::new(ContainerManager::new()?);
    let reconcile_interval_ms: u64 = std::env::var("EXO_RECONCILE_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000);
    // Opt-in auto-removal of exited containers and their overlay artifacts.
    // Default `None` (off) preserves the user's expectation that stopped
    // containers stick around until `exo rm`. Setting this is the
    // recommended switch at scale to keep disk usage bounded.
    let stale_after = std::env::var("EXO_STALE_AFTER_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs);
    if let Some(d) = stale_after {
        tracing::info!("auto-removing exited containers older than {:?}", d);
    }
    let reconciler = Reconciler::new(
        manager.clone(),
        events.clone(),
        ReconcileOptions {
            interval: Duration::from_millis(reconcile_interval_ms),
            stale_after,
            ..Default::default()
        },
    );

    // Run startup recovery pass BEFORE binding the socket so that the first
    // connection sees a coherent world (no orphan cgroups, no zombie metadata).
    let _summary = reconciler.run_recovery_pass();

    // Bind the socket.
    let listener = UnixListener::bind(&config.socket_path)?;
    println!("Daemon listening on {}", config.socket_path);
    println!("Ready to accept connections");

    let mut perms = std::fs::metadata(&config.socket_path)?.permissions();
    perms.set_mode(0o777);
    std::fs::set_permissions(&config.socket_path, perms)?;

    // Spawn the periodic reconcile loop on the tokio runtime (the binary is
    // launched under #[tokio::main], so a runtime is already live). Also
    // stash the handle for sync worker threads (image-pull block_on).
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let _ = RUNTIME_HANDLE.set(handle.clone());
        let rec = reconciler.clone();
        handle.spawn(async move { rec.run_loop().await });
    } else {
        tracing::warn!(
            "no current tokio runtime; periodic reconciler disabled and image pulls will fail"
        );
    }

    // Build admission-control semaphores.
    let max_starts = std::env::var("EXO_MAX_CONCURRENT_STARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_CONCURRENT_STARTS);
    let max_conns = std::env::var("EXO_MAX_CONCURRENT_CONNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_CONCURRENT_CONNS);
    let start_sem = Arc::new(Semaphore::new(max_starts));
    let conn_sem = Arc::new(Semaphore::new(max_conns));
    tracing::info!(
        "admission caps: max_concurrent_starts={}, max_concurrent_conns={}",
        max_starts,
        max_conns
    );

    // Spawn a background thread to handle connections.
    let state = Arc::new(Mutex::new(DaemonState::default()));
    let config_clone = config.clone();
    let events_for_server = events.clone();

    thread::spawn(move || {
        if let Err(e) = run_server(
            listener,
            state,
            config_clone,
            events_for_server,
            start_sem,
            conn_sem,
        ) {
            eprintln!("Daemon error: {}", e);
        }
    });

    // Keep main thread alive.
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

/// Daemon state tracking containers and resources
#[derive(Debug, Default)]
struct DaemonState {
    containers: Vec<ContainerInfo>,
}

#[derive(Debug, Clone)]
struct ContainerInfo {
    id: String,
    name: String,
    pid: Option<u32>,
    status: String,
}

/// Run the server loop
#[cfg(all(unix, not(target_os = "macos")))]
fn run_server(
    listener: UnixListener,
    state: Arc<Mutex<DaemonState>>,
    config: DaemonConfig,
    events: Option<EventLog>,
    start_sem: Arc<Semaphore>,
    conn_sem: Arc<Semaphore>,
) -> Result<()> {
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                // Connection-cap admission: try to acquire an *owned* permit
                // we can move into the worker thread. If we're at the cap,
                // write a busy response inline (no thread spawn) and close.
                let conn_permit = match conn_sem.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        let resp = DaemonResponse::Error {
                            message: "daemon at connection capacity, retry later".to_string(),
                        };
                        if let Ok(json) = serde_json::to_string(&resp) {
                            let _ = stream.write_all(json.as_bytes());
                            let _ = stream.write_all(b"\n");
                        }
                        tracing::warn!("dropped connection: at conn capacity");
                        continue;
                    }
                };

                let state = state.clone();
                let config_clone = config.clone();
                let events_clone = events.clone();
                let start_sem_clone = start_sem.clone();
                thread::spawn(move || {
                    let _conn_permit = conn_permit; // hold for connection lifetime
                    if let Err(e) = handle_client(
                        stream,
                        state,
                        &config_clone,
                        events_clone.as_ref(),
                        &start_sem_clone,
                    ) {
                        eprintln!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Connection failed: {}", e);
            }
        }
    }
    Ok(())
}

/// Handle a single client connection
#[cfg(all(unix, not(target_os = "macos")))]
fn handle_client(
    mut stream: UnixStream,
    _state: Arc<Mutex<DaemonState>>,
    config: &DaemonConfig,
    events: Option<&EventLog>,
    start_sem: &Arc<Semaphore>,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(config.timeout_ms)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();

    // Read request (single line JSON)
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }

    let request: DaemonRequest = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => {
            let error_response = DaemonResponse::Error {
                message: format!("Invalid request: {}", e),
            };
            let response_json = serde_json::to_string(&error_response)?;
            stream.write_all(response_json.as_bytes())?;
            stream.write_all(b"\n")?;
            stream.flush()?;
            return Ok(());
        }
    };

    // Process request
    let response: DaemonResponse = match request {
        DaemonRequest::Run { spec } => execute_run(&spec, events, start_sem),
        DaemonRequest::Stop { container_id } => execute_stop(&container_id, events),
        DaemonRequest::List { all } => execute_list(all),
        DaemonRequest::Status { container_id } => execute_status(&container_id),
        DaemonRequest::Ping => DaemonResponse::Pong,
        DaemonRequest::Shutdown => DaemonResponse::Ok {
            message: "Shutting down".to_string(),
        },
    };

    // Send response
    let response_json = serde_json::to_string(&response)?;
    stream.write_all(response_json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn execute_run(
    spec: &ContainerSpec,
    events: Option<&EventLog>,
    start_sem: &Arc<Semaphore>,
) -> DaemonResponse {
    use exo_runtime::config::{ContainerConfig, MountConfig};

    // Concurrent-starts admission. Sync `try_acquire` — no runtime needed.
    // Permit drops when the function returns. Cap is set via
    // EXO_MAX_CONCURRENT_STARTS at daemon startup.
    let _start_permit = match start_sem.try_acquire() {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!("rejected start of {}: at concurrency cap", spec.name);
            if let Some(log) = events {
                let _ = log.record(
                    &spec.name,
                    &spec.name,
                    EventType::Failed,
                    Some("admission: start-concurrency cap"),
                );
            }
            return DaemonResponse::Error {
                message: "daemon at start-concurrency cap; retry shortly".to_string(),
            };
        }
    };

    // Build ContainerConfig from ContainerSpec
    let mut env_map: HashMap<String, String> = HashMap::new();
    for env_var in &spec.env {
        if let Some((k, v)) = env_var.split_once('=') {
            env_map.insert(k.to_string(), v.to_string());
        }
    }

    let mut mounts_vec = vec![];
    for mount in &spec.mounts {
        mounts_vec.push(MountConfig {
            mount_type: "bind".to_string(),
            source: mount.source.clone(),
            target: mount.target.clone(),
            readonly: mount.readonly,
            size: None,
            propagation: "rprivate".to_string(),
        });
    }

    let config = ContainerConfig {
        name: spec.name.clone(),
        image: spec.image.clone(),
        workdir: PathBuf::from(&spec.workdir),
        env: env_map,
        user: "root".to_string(),
        command: spec.command.clone(),
        resources: Default::default(),
        network: Default::default(),
        mounts: mounts_vec,
        gpu: None,
        ..Default::default()
    };

    let manager = match ContainerManager::new() {
        Ok(m) => m,
        Err(e) => {
            if let Some(log) = events {
                let _ = log.record(
                    &spec.name,
                    &spec.name,
                    EventType::Failed,
                    Some(&format!("manager init: {}", e)),
                );
            }
            return DaemonResponse::Error {
                message: format!("Failed to open state dir: {}", e),
            };
        }
    };

    if manager.exists(&spec.name) {
        if let Some(log) = events {
            let _ = log.record(
                &spec.name,
                &spec.name,
                EventType::Failed,
                Some("name already in use"),
            );
        }
        return DaemonResponse::Error {
            message: format!("Container name already in use: {}", spec.name),
        };
    }

    // Ensure the image rootfs is ready *before* Container::new looks for it.
    // This is the daemon's equivalent of run.rs::execute_linux's ensure_rootfs
    // call. Without this, agents whose image hasn't been pre-pulled fail
    // silently inside Container::new with a confusing rootfs error.
    if let Err(e) = ensure_image_rootfs(&spec.image) {
        if let Some(log) = events {
            let _ = log.record(
                &spec.name,
                &spec.name,
                EventType::Failed,
                Some(&format!("image pull: {}", e)),
            );
        }
        return DaemonResponse::Error {
            message: format!("Failed to prepare image '{}': {}", spec.image, e),
        };
    }

    let mut metadata = ContainerMetadata::new(spec.name.clone(), config.clone());
    // Stable identifier for events from this point until Container::new
    // assigns its own id. Events emitted before the swap use this; after
    // start succeeds, metadata.id is replaced with container.handle().id
    // and Started is emitted under that one. Both are linked by container_name.
    let pre_id = metadata.id.clone();

    if let Some(log) = events {
        let _ = log.record(&pre_id, &spec.name, EventType::Created, None);
    }

    let mut container = match Container::new(config) {
        Ok(c) => c,
        Err(e) => {
            if let Some(log) = events {
                let _ = log.record(
                    &pre_id,
                    &spec.name,
                    EventType::Failed,
                    Some(&format!("Container::new: {}", e)),
                );
            }
            return DaemonResponse::Error {
                message: format!("Failed to create container: {}", e),
            };
        }
    };

    if let Err(e) = container.start() {
        if let Some(log) = events {
            let _ = log.record(
                &pre_id,
                &spec.name,
                EventType::Failed,
                Some(&format!("Container::start: {}", e)),
            );
        }
        return DaemonResponse::Error {
            message: format!("Failed to start container: {}", e),
        };
    }

    let id = container.handle().id.clone();
    metadata.id = id.clone();
    if let Some(pid) = container.handle().pid {
        metadata.set_running(pid);
    }

    if let Err(e) = manager.save(&metadata) {
        // Container is running but metadata failed to persist — log and continue.
        // The reconciler will adopt the orphan via cgroup-based detection.
        tracing::warn!(
            "Container {} started but metadata save failed: {}",
            spec.name,
            e
        );
        if let Some(log) = events {
            let _ = log.record(
                &id,
                &spec.name,
                EventType::Failed,
                Some(&format!("metadata save: {}", e)),
            );
        }
    }

    if let Some(log) = events {
        let detail = metadata.pid.map(|p| format!("pid={}", p));
        let _ = log.record(&id, &spec.name, EventType::Started, detail.as_deref());
    }

    DaemonResponse::Ok {
        message: format!("Container started: {} (id={})", spec.name, id),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn execute_stop(container_id: &str, events: Option<&EventLog>) -> DaemonResponse {
    let manager = match ContainerManager::new() {
        Ok(m) => m,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!("Failed to open state dir: {}", e),
            };
        }
    };

    let mut metadata = match manager.find(container_id) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return DaemonResponse::Error {
                message: format!("Container not found: {}", container_id),
            };
        }
        Err(e) => {
            return DaemonResponse::Error {
                message: format!("Lookup failed: {}", e),
            };
        }
    };

    if !metadata.is_running() {
        return DaemonResponse::Ok {
            message: format!(
                "Container {} is not running (status: {})",
                metadata.name, metadata.status
            ),
        };
    }

    let pid = match metadata.pid {
        Some(p) => p,
        None => {
            return DaemonResponse::Error {
                message: format!("Container {} has no PID", metadata.name),
            };
        }
    };

    if let Some(log) = events {
        let _ = log.record(&metadata.id, &metadata.name, EventType::StopRequested, None);
    }

    // SIGTERM, then SIGKILL after a short grace period.
    if let Err(e) = send_signal(pid, libc::SIGTERM) {
        return DaemonResponse::Error {
            message: format!("Failed to signal {}: {}", pid, e),
        };
    }
    let killed_hard = if !wait_for_exit(pid, Duration::from_secs(5)) {
        let _ = send_signal(pid, libc::SIGKILL);
        thread::sleep(Duration::from_millis(100));
        true
    } else {
        false
    };

    metadata.set_stopped(None);
    if let Err(e) = manager.save(&metadata) {
        tracing::warn!("Stopped {} but metadata save failed: {}", metadata.name, e);
    }

    if let Some(log) = events {
        let detail = if killed_hard {
            Some("sigkill")
        } else {
            Some("sigterm")
        };
        let _ = log.record(&metadata.id, &metadata.name, EventType::Killed, detail);
    }

    DaemonResponse::Ok {
        message: format!("Container stopped: {}", metadata.name),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn execute_list(all: bool) -> DaemonResponse {
    let manager = match ContainerManager::new() {
        Ok(m) => m,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!("Failed to open state dir: {}", e),
            };
        }
    };

    let mut containers = match manager.list() {
        Ok(c) => c,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!("List failed: {}", e),
            };
        }
    };

    // Refresh status for each (cheap /proc check).
    for c in &mut containers {
        let _ = manager.refresh_status(c);
    }

    if !all {
        containers.retain(|c| c.is_running());
    }

    let json_containers: Vec<ContainerJson> =
        containers.into_iter().map(ContainerJson::from).collect();
    let payload = ContainerListJson {
        containers: json_containers,
    };

    match serde_json::to_string(&payload) {
        Ok(s) => DaemonResponse::List { containers: s },
        Err(e) => DaemonResponse::Error {
            message: format!("Failed to serialize list: {}", e),
        },
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn execute_status(container_id: &str) -> DaemonResponse {
    let manager = match ContainerManager::new() {
        Ok(m) => m,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!("Failed to open state dir: {}", e),
            };
        }
    };

    match manager.find(container_id) {
        Ok(Some(mut m)) => {
            let _ = manager.refresh_status(&mut m);
            DaemonResponse::Status {
                container: m.name,
                status: m.status,
            }
        }
        Ok(None) => DaemonResponse::Status {
            container: container_id.to_string(),
            status: "unknown".to_string(),
        },
        Err(e) => DaemonResponse::Error {
            message: format!("Lookup failed: {}", e),
        },
    }
}

/// Prepare the image rootfs for `image` before `Container::new` looks for it.
///
/// 1. Parse the image reference.
/// 2. `block_on` `ImageManager::ensure_rootfs` (no-op if already extracted).
/// 3. Symlink the resulting rootfs into `/tmp/exo-images/rootfs/<sanitized>`
///    where the runtime expects to find it (see `rootfs.rs::prepare_rootfs`).
///
/// Without the `registry` feature, this still verifies the rootfs is present
/// locally and returns a clear error if not — which is strictly better than
/// the current silent failure inside `Container::new`.
#[cfg(all(unix, not(target_os = "macos")))]
fn ensure_image_rootfs(image: &str) -> Result<()> {
    use exo_image::ImageManager;

    let runtime = RUNTIME_HANDLE
        .get()
        .ok_or_else(|| anyhow::anyhow!("daemon runtime handle not initialized"))?;

    let manager = ImageManager::new()?;
    let image_ref = manager.parse_image_reference(image)?;

    // Where the runtime looks for the prepared rootfs.
    let link_path = PathBuf::from("/tmp/exo-images/rootfs").join(image.replace(['/', ':'], "_"));

    // Fast path: symlink already in place from a previous run.
    if link_path.exists() && link_path.join("bin").exists() {
        tracing::debug!("Image rootfs already prepared at {:?}", link_path);
        return Ok(());
    }

    // Run the async ensure_rootfs from this sync worker thread.
    let actual = runtime.block_on(manager.ensure_rootfs(&image_ref))?;

    // Make the well-known path point at the actual rootfs. Idempotent:
    // skip if it's already there.
    if !link_path.exists() {
        if let Some(parent) = link_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(e) = std::os::unix::fs::symlink(&actual, &link_path) {
            // EEXIST is fine — race between two concurrent starts of the
            // same image both racing to create the symlink. Anything else
            // is a real error.
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(e).map_err(|e| {
                    anyhow::anyhow!("symlink {:?} -> {:?}: {}", link_path, actual, e)
                })?;
            }
        }
        tracing::info!("Prepared image rootfs: {:?} -> {:?}", link_path, actual);
    }

    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn send_signal(pid: u32, signal: libc::c_int) -> Result<()> {
    let rc = unsafe { libc::kill(pid as i32, signal) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        // ESRCH is fine — process already gone.
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(err.into());
        }
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    let proc_path = format!("/proc/{}", pid);
    while start.elapsed() < timeout {
        if !std::path::Path::new(&proc_path).exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    !std::path::Path::new(&proc_path).exists()
}

/// Transitional: the daemon protocol reports errors as strings; map its
/// stable message shapes onto the typed taxonomy (cf. `map_guest_error` in
/// exo-vm-mac) until the protocol carries typed codes. Unknown shapes pass
/// through as untyped anyhow errors (exit 6).
#[cfg(all(unix, not(target_os = "macos")))]
pub fn map_daemon_error(message: String) -> anyhow::Error {
    if let Some(name) = message.strip_prefix("Container name already in use: ") {
        return exo_runtime::ExoError::ContainerAlreadyExists(name.trim().to_string()).into();
    }
    if let Some(name) = message.strip_prefix("Container not found: ") {
        return exo_runtime::ExoError::ContainerNotFound(name.trim().to_string()).into();
    }
    if message.starts_with("daemon at start-concurrency cap")
        || message.starts_with("daemon at connection capacity")
    {
        return exo_runtime::ExoError::BackendUnavailable(message).into();
    }
    anyhow::anyhow!(message)
}

/// Daemon request types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
enum DaemonRequest {
    #[serde(rename = "run")]
    Run { spec: ContainerSpec },

    #[serde(rename = "stop")]
    Stop { container_id: String },

    #[serde(rename = "list")]
    List { all: bool },

    #[serde(rename = "status")]
    Status { container_id: String },

    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "shutdown")]
    Shutdown,
}

/// Daemon response types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
enum DaemonResponse {
    #[serde(rename = "ok")]
    Ok { message: String },

    #[serde(rename = "error")]
    Error { message: String },

    #[serde(rename = "list")]
    List { containers: String },

    #[serde(rename = "status")]
    Status { container: String, status: String },

    #[serde(rename = "pong")]
    Pong,
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod daemon_error_tests {
    use super::map_daemon_error;

    #[test]
    fn daemon_messages_map_to_taxonomy() {
        let err = map_daemon_error("Container name already in use: web".to_string());
        assert_eq!(exo_runtime::exit_code_for(&err), 3);

        let err = map_daemon_error("Container not found: ghost".to_string());
        assert_eq!(exo_runtime::exit_code_for(&err), 2);

        let err = map_daemon_error("daemon at start-concurrency cap; retry shortly".to_string());
        assert_eq!(exo_runtime::exit_code_for(&err), 4);
        assert!(err
            .downcast_ref::<exo_runtime::ExoError>()
            .unwrap()
            .retryable());
    }

    #[test]
    fn unknown_daemon_messages_stay_internal() {
        let err = map_daemon_error("Failed to create container: boom".to_string());
        assert_eq!(exo_runtime::exit_code_for(&err), 6);
    }
}
