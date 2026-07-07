use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VmStatus {
    Created,
    Running,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmState {
    pub name: String,
    pub id: String,
    pub status: VmStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub kernel_path: PathBuf,
    pub initrd_path: PathBuf,
    pub disk_path: PathBuf,
}

impl VmState {
    pub fn new(name: &str, id: &str, kernel: PathBuf, initrd: PathBuf, disk: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            id: id.to_string(),
            status: VmStatus::Created,
            created_at: Utc::now(),
            started_at: None,
            kernel_path: kernel,
            initrd_path: initrd,
            disk_path: disk,
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let state = serde_json::from_str(&contents)?;
        Ok(state)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(path, contents)?;
        Ok(())
    }

    pub fn set_status(&mut self, status: VmStatus) {
        self.status = status;
    }
}
