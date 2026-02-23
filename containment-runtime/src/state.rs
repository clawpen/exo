//! Container state persistence.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Container state information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerState {
    pub id: String,
    pub name: String,
    pub image: String,
    pub pid: Option<u32>,
    pub status: String,
    pub created_at: i64,
}

/// Save container state to disk.
pub fn save_state(id: &str, status: &str) -> Result<()> {
    let state = ContainerState {
        id: id.to_string(),
        name: "container".to_string(),
        image: "unknown".to_string(),
        pid: None,
        status: status.to_string(),
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() as i64,
    };

    let state_file = state_dir(id).join("state.json");
    let state_json = serde_json::to_string_pretty(&state)?;

    fs::write(state_file, state_json)?;
    update_index(id, &state)?;

    Ok(())
}

/// Load container state from disk.
pub fn load_container(id: &str) -> Result<Option<ContainerState>> {
    let state_file = state_dir(id).join("state.json");

    if !state_file.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(state_file)?;
    let state: ContainerState = serde_json::from_str(&content)?;

    Ok(Some(state))
}

/// Update container status.
pub fn update_status(id: &str, status: &str) -> Result<()> {
    if let Some(mut state) = load_container(id)? {
        state.status = status.to_string();

        let state_file = state_dir(id).join("state.json");
        fs::write(state_file, serde_json::to_string_pretty(&state)?)?;

        update_index(id, &state)?;
    }

    Ok(())
}

/// Get container status.
pub fn get_status(id: &str) -> Result<Option<String>> {
    load_container(id).map(|s| s.map(|s| s.status))
}

/// Get the state directory for a container.
fn state_dir(id: &str) -> PathBuf {
    PathBuf::from(format!("/var/lib/openclaw/containers/{}", id))
}

/// Get the global containers index.
fn index_file() -> PathBuf {
    PathBuf::from("/var/lib/openclaw/containers/index.json")
}

/// Update the global container index.
fn update_index(id: &str, state: &ContainerState) -> Result<()> {
    let index_path = index_file();

    let mut index: HashMap<String, ContainerState> = if index_path.exists() {
        let content = fs::read_to_string(&index_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        HashMap::new()
    };

    index.insert(id.to_string(), state.clone());

    fs::create_dir_all(index_path.parent().unwrap())?;
    fs::write(&index_path, serde_json::to_string_pretty(&index)?)?;

    Ok(())
}

/// List all containers.
pub fn list_containers() -> Result<HashMap<String, ContainerInfo>> {
    let index_path = index_file();

    if !index_path.exists() {
        return Ok(HashMap::new());
    }

    let content = fs::read_to_string(&index_path)?;
    let index: HashMap<String, ContainerState> = serde_json::from_str(&content)?;

    let result: HashMap<String, ContainerInfo> = index
        .into_iter()
        .map(|(id, state)| {
            let info = ContainerInfo {
                name: state.name,
                image: state.image,
                status: state.status,
            };
            (id, info)
        })
        .collect();

    Ok(result)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_dir() {
        let dir = state_dir("test-id");
        assert!(dir.to_str().unwrap().contains("test-id"));
    }
}
