//! Persistent orchestration run store.
//!
//! Stores orchestration state and append-only events so an external
//! orchestrator (Orchestre) can inspect, resume, and audit runs.

use crate::orchestrator::OrchestrationState;
use crate::runner::RunOutcome;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub state: OrchestrationState,
    pub outcome: Option<RunOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub timestamp_ms: u128,
    pub run_id: String,
    pub event_type: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct RunStore {
    root: PathBuf,
}

impl RunStore {
    pub fn new_default() -> Result<Self> {
        let root = if let Ok(dir) = std::env::var("EXO_ORCHESTRATION_DIR") {
            PathBuf::from(dir)
        } else if let Ok(state) = std::env::var("EXO_STATE_DIR") {
            PathBuf::from(state).join("orchestrations")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("exo")
                .join("orchestrations")
        } else {
            std::env::temp_dir().join("exo").join("orchestrations")
        };
        Self::new(root)
    }

    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.root.join(run_id)
    }

    pub fn state_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("state.json")
    }

    pub fn events_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("events.jsonl")
    }

    pub fn artifacts_dir(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("artifacts")
    }

    pub fn save(&self, record: &RunRecord) -> Result<()> {
        let dir = self.run_dir(&record.run_id);
        std::fs::create_dir_all(&dir)?;
        std::fs::create_dir_all(self.artifacts_dir(&record.run_id))?;
        std::fs::write(
            self.state_path(&record.run_id),
            serde_json::to_vec_pretty(record)?,
        )
        .with_context(|| format!("write run state {}", record.run_id))?;
        Ok(())
    }

    pub fn load(&self, run_id: &str) -> Result<RunRecord> {
        let bytes = std::fs::read(self.state_path(run_id))
            .with_context(|| format!("read run state {}", run_id))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn append_event(&self, run_id: &str, event_type: &str, message: &str) -> Result<()> {
        let dir = self.run_dir(run_id);
        std::fs::create_dir_all(&dir)?;
        let event = RunEvent {
            timestamp_ms: now_ms(),
            run_id: run_id.to_string(),
            event_type: event_type.to_string(),
            message: message.to_string(),
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.events_path(run_id))?;
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
        Ok(())
    }
}

pub fn new_run_id() -> String {
    format!("orch-{}", uuid::Uuid::new_v4())
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{default_agent_roles, Orchestrator, PrimeDirective};

    fn record() -> RunRecord {
        let directive = PrimeDirective {
            objective: "test".to_string(),
            success_criteria: vec![],
            constraints: vec![],
            max_rounds: 3,
        };
        let state = Orchestrator::new(directive, default_agent_roles())
            .state()
            .clone();
        RunRecord {
            run_id: "run-1".to_string(),
            state,
            outcome: None,
        }
    }

    #[test]
    fn save_load_and_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::new(dir.path()).unwrap();
        let rec = record();
        store.save(&rec).unwrap();
        store
            .append_event(&rec.run_id, "started", "run started")
            .unwrap();

        let loaded = store.load(&rec.run_id).unwrap();
        assert_eq!(loaded.run_id, rec.run_id);
        let events = std::fs::read_to_string(store.events_path(&rec.run_id)).unwrap();
        assert!(events.contains("\"event_type\":\"started\""));
        assert!(store.artifacts_dir(&rec.run_id).is_dir());
    }
}
