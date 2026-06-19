//! Container manager for persistent container state.
//!
//! Manages container metadata storage and lifecycle operations.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::ContainerConfig;

/// Default container state directory (system-wide)
pub const CONTAINER_STATE_DIR: &str = "/var/lib/exo/containers";

/// Fallback state directory (user-local)
pub const FALLBACK_STATE_DIR: &str = ".local/share/exo/containers";

/// Container metadata stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerMetadata {
    /// Unique container ID
    pub id: String,

    /// Container name
    pub name: String,

    /// Image used
    pub image: String,

    /// Current status
    pub status: String,

    /// Process ID (if running)
    pub pid: Option<u32>,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last started timestamp
    pub started_at: Option<DateTime<Utc>>,

    /// Last stopped timestamp
    pub stopped_at: Option<DateTime<Utc>>,

    /// Exit code (if exited)
    pub exit_code: Option<i32>,

    /// Full container configuration (for restart)
    pub config: ContainerConfig,

    /// Port mappings for display
    pub ports: Vec<String>,

    /// Custom labels/tags
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

impl ContainerMetadata {
    /// Create new metadata for a container.
    pub fn new(name: String, config: ContainerConfig) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            image: config.image.clone(),
            status: "created".to_string(),
            pid: None,
            created_at: Utc::now(),
            started_at: None,
            stopped_at: None,
            exit_code: None,
            config,
            ports: vec![],
            labels: HashMap::new(),
        }
    }

    /// Mark container as running.
    pub fn set_running(&mut self, pid: u32) {
        self.status = "running".to_string();
        self.pid = Some(pid);
        self.started_at = Some(Utc::now());
        self.exit_code = None;
    }

    /// Mark container as stopped.
    pub fn set_stopped(&mut self, exit_code: Option<i32>) {
        self.status = "exited".to_string();
        self.pid = None;
        self.stopped_at = Some(Utc::now());
        self.exit_code = exit_code;
    }

    /// Check if container is running.
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }

    /// Get container directory path.
    pub fn dir_path(&self) -> PathBuf {
        PathBuf::from(CONTAINER_STATE_DIR).join(&self.name)
    }

    /// Get config file path.
    pub fn config_path(&self) -> PathBuf {
        self.dir_path().join("config.json")
    }
}

/// Container manager for persistent storage.
pub struct ContainerManager {
    state_dir: PathBuf,
}

impl ContainerManager {
    /// Create a new container manager.
    /// 
    /// Tries to use the system-wide state directory first, falling back
    /// to a user-local directory if the system directory isn't accessible.
    pub fn new() -> Result<Self> {
        // Try system directory first
        if let Ok(manager) = Self::with_state_dir(CONTAINER_STATE_DIR) {
            return Ok(manager);
        }
        
        // Fall back to user-local directory
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        
        let fallback_dir = PathBuf::from(home).join(FALLBACK_STATE_DIR);
        Self::with_state_dir(&fallback_dir)
    }

    /// Create a manager with a custom state directory.
    pub fn with_state_dir(state_dir: impl AsRef<Path>) -> Result<Self> {
        let state_dir = state_dir.as_ref().to_path_buf();
        
        // Ensure state directory exists
        fs::create_dir_all(&state_dir)
            .with_context(|| format!("Failed to create state directory: {:?}", state_dir))?;
        
        Ok(Self { state_dir })
    }

    /// Get the state directory path.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Save container metadata.
    pub fn save(&self, metadata: &ContainerMetadata) -> Result<()> {
        let container_dir = self.state_dir.join(&metadata.name);
        fs::create_dir_all(&container_dir)
            .with_context(|| format!("Failed to create container directory: {:?}", container_dir))?;
        
        let config_path = container_dir.join("config.json");
        let json = serde_json::to_string_pretty(metadata)
            .context("Failed to serialize container metadata")?;
        
        fs::write(&config_path, json)
            .with_context(|| format!("Failed to write container config: {:?}", config_path))?;
        
        // Write PID file if running
        if let Some(pid) = metadata.pid {
            let pid_path = container_dir.join("pid");
            fs::write(&pid_path, pid.to_string())
                .with_context(|| format!("Failed to write PID file: {:?}", pid_path))?;
        }
        
        tracing::debug!("Saved container metadata: {}", metadata.name);
        Ok(())
    }

    /// Load container metadata by name.
    pub fn load(&self, name: &str) -> Result<ContainerMetadata> {
        let config_path = self.state_dir.join(name).join("config.json");
        
        let json = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read container config for: {}", name))?;
        
        let metadata: ContainerMetadata = serde_json::from_str(&json)
            .with_context(|| format!("Failed to parse container config for: {}", name))?;
        
        Ok(metadata)
    }

    /// Check if a container exists.
    pub fn exists(&self, name: &str) -> bool {
        self.state_dir.join(name).join("config.json").exists()
    }

    /// List all containers.
    pub fn list(&self) -> Result<Vec<ContainerMetadata>> {
        let mut containers = Vec::new();
        
        let entries = match fs::read_dir(&self.state_dir) {
            Ok(entries) => entries,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Ok(containers);
                }
                return Err(e).context("Failed to read state directory");
            }
        };
        
        for entry in entries {
            let entry = entry.context("Failed to read directory entry")?;
            let path = entry.path();
            
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    match self.load(name) {
                        Ok(metadata) => containers.push(metadata),
                        Err(e) => {
                            tracing::warn!("Failed to load container {}: {}", name, e);
                        }
                    }
                }
            }
        }
        
        // Sort by creation time, newest first
        containers.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        
        Ok(containers)
    }

    /// List only running containers.
    pub fn list_running(&self) -> Result<Vec<ContainerMetadata>> {
        let containers = self.list()?;
        Ok(containers.into_iter().filter(|c| c.is_running()).collect())
    }

    /// Remove container metadata.
    pub fn remove(&self, name: &str) -> Result<()> {
        let container_dir = self.state_dir.join(name);
        
        if container_dir.exists() {
            fs::remove_dir_all(&container_dir)
                .with_context(|| format!("Failed to remove container directory: {:?}", container_dir))?;
        }
        
        tracing::info!("Removed container: {}", name);
        Ok(())
    }

    /// Update container status by checking if process is alive.
    pub fn refresh_status(&self, metadata: &mut ContainerMetadata) -> Result<()> {
        if let Some(pid) = metadata.pid {
            // Check if process is still alive
            let proc_path = PathBuf::from("/proc").join(pid.to_string());
            
            if proc_path.exists() {
                // Process is still running
                if metadata.status != "running" {
                    metadata.status = "running".to_string();
                    self.save(metadata)?;
                }
            } else {
                // Process has exited
                if metadata.status == "running" {
                    metadata.status = "exited".to_string();
                    metadata.pid = None;
                    metadata.stopped_at = Some(Utc::now());
                    // Try to read exit code from previous run (would need to store this)
                    self.save(metadata)?;
                }
            }
        }
        
        Ok(())
    }

    /// Find container by name or ID prefix.
    pub fn find(&self, name_or_id: &str) -> Result<Option<ContainerMetadata>> {
        // First try exact name match
        if self.exists(name_or_id) {
            return Ok(Some(self.load(name_or_id)?));
        }
        
        // Try ID prefix match
        let containers = self.list()?;
        for container in containers {
            if container.id.starts_with(name_or_id) {
                return Ok(Some(container));
            }
        }
        
        Ok(None)
    }

    /// Generate a unique container name.
    pub fn generate_name(&self) -> String {
        loop {
            let id = Uuid::new_v4().to_string();
            let name = format!("exo-{}", &id[..8]);
            if !self.exists(&name) {
                return name;
            }
        }
    }

    /// Get the count of running containers.
    pub fn running_count(&self) -> Result<usize> {
        let containers = self.list_running()?;
        Ok(containers.len())
    }

    /// Clean up stale container entries (exited containers older than max_age).
    pub fn cleanup_stale(&self, max_age_hours: u64) -> Result<Vec<String>> {
        let containers = self.list()?;
        let mut removed = Vec::new();
        let cutoff = Utc::now() - chrono::Duration::hours(max_age_hours as i64);
        
        for container in containers {
            if !container.is_running() {
                if let Some(stopped_at) = container.stopped_at {
                    if stopped_at < cutoff {
                        self.remove(&container.name)?;
                        removed.push(container.name);
                    }
                }
            }
        }
        
        Ok(removed)
    }
}

impl Default for ContainerManager {
    fn default() -> Self {
        Self::new().expect("Failed to create ContainerManager")
    }
}

/// JSON output format for `exo list --json`
#[derive(Debug, Serialize)]
pub struct ContainerListJson {
    pub containers: Vec<ContainerJson>,
}

/// Single container in JSON output.
#[derive(Debug, Serialize)]
pub struct ContainerJson {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub pid: Option<u32>,
    pub created: DateTime<Utc>,
    pub ports: Vec<String>,
}

impl From<ContainerMetadata> for ContainerJson {
    fn from(meta: ContainerMetadata) -> Self {
        Self {
            id: meta.id,
            name: meta.name,
            image: meta.image,
            status: meta.status,
            pid: meta.pid,
            created: meta.created_at,
            ports: meta.ports,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_manager() -> (ContainerManager, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let manager = ContainerManager::with_state_dir(temp_dir.path())
            .expect("Failed to create manager");
        (manager, temp_dir)
    }

    fn test_config() -> ContainerConfig {
        ContainerConfig {
            name: "test".to_string(),
            image: "alpine:latest".to_string(),
            command: vec!["sleep".to_string(), "60".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn test_create_and_save() {
        let (manager, _temp) = test_manager();
        
        let config = test_config();
        let metadata = ContainerMetadata::new("test-container".to_string(), config);
        
        assert!(manager.save(&metadata).is_ok());
        assert!(manager.exists("test-container"));
    }

    #[test]
    fn test_load() {
        let (manager, _temp) = test_manager();
        
        let config = test_config();
        let metadata = ContainerMetadata::new("test-load".to_string(), config.clone());
        manager.save(&metadata).unwrap();
        
        let loaded = manager.load("test-load").unwrap();
        assert_eq!(loaded.name, "test-load");
        assert_eq!(loaded.image, "alpine:latest");
    }

    #[test]
    fn test_list() {
        let (manager, _temp) = test_manager();
        
        let config = test_config();
        
        let m1 = ContainerMetadata::new("container-1".to_string(), config.clone());
        let m2 = ContainerMetadata::new("container-2".to_string(), config.clone());
        
        manager.save(&m1).unwrap();
        manager.save(&m2).unwrap();
        
        let list = manager.list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_remove() {
        let (manager, _temp) = test_manager();
        
        let config = test_config();
        let metadata = ContainerMetadata::new("to-remove".to_string(), config);
        
        manager.save(&metadata).unwrap();
        assert!(manager.exists("to-remove"));
        
        manager.remove("to-remove").unwrap();
        assert!(!manager.exists("to-remove"));
    }

    #[test]
    fn test_status_transitions() {
        let mut metadata = ContainerMetadata::new("test".to_string(), test_config());
        
        assert_eq!(metadata.status, "created");
        assert!(!metadata.is_running());
        
        metadata.set_running(1234);
        assert_eq!(metadata.status, "running");
        assert!(metadata.is_running());
        assert_eq!(metadata.pid, Some(1234));
        
        metadata.set_stopped(Some(0));
        assert_eq!(metadata.status, "exited");
        assert!(!metadata.is_running());
        assert_eq!(metadata.exit_code, Some(0));
    }

    #[test]
    fn test_find_by_id_prefix() {
        let (manager, _temp) = test_manager();
        
        let config = test_config();
        let metadata = ContainerMetadata::new("find-test".to_string(), config);
        let id = metadata.id.clone();
        
        manager.save(&metadata).unwrap();
        
        // Find by full ID
        let found = manager.find(&id).unwrap();
        assert!(found.is_some());
        
        // Find by short prefix
        let found = manager.find(&id[..8]).unwrap();
        assert!(found.is_some());
        
        // Find by name
        let found = manager.find("find-test").unwrap();
        assert!(found.is_some());
    }
}
