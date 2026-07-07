//! Persistent orchestration run store.
//!
//! Stores orchestration state and append-only events so an external
//! orchestrator (Orchestre) can inspect, resume, and audit runs.

use crate::orchestrator::OrchestrationState;
use crate::runner::RunOutcome;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long to wait for a run's append lock before giving up.
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// A lock file older than this is treated as stale and reclaimed.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);

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

/// Durable inter-agent/coordinator event.
///
/// The mailbox is append-only and ordered by `sequence`. Agents and Orchestre
/// can read it after a restart to reconstruct handoffs, checkpoints, sleep/wake
/// notices, and task reports without needing a live daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MailboxEvent {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub run_id: String,
    pub event_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub message: String,
    #[serde(default)]
    pub payload: Value,
}

impl MailboxEvent {
    pub fn new(
        run_id: impl Into<String>,
        kind: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            sequence: 0,
            timestamp_ms: 0,
            run_id: run_id.into(),
            event_id: String::new(),
            kind: kind.into(),
            from: None,
            to: None,
            task_id: None,
            message: message.into(),
            payload: Value::Null,
        }
    }

    pub fn from(mut self, from: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self
    }

    pub fn to(mut self, to: impl Into<String>) -> Self {
        self.to = Some(to.into());
        self
    }

    pub fn task_id(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }
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

    pub fn mailbox_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("mailbox.jsonl")
    }

    pub fn mailbox_seq_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("mailbox.seq")
    }

    pub fn lock_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join(".append.lock")
    }

    pub fn input_path(&self, run_id: &str) -> PathBuf {
        self.run_dir(run_id).join("input.json")
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

    pub fn list_run_ids(&self) -> Result<Vec<String>> {
        let mut ids = vec![];
        if !self.root.exists() {
            return Ok(ids);
        }
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if self.state_path(&id).exists() {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn save_input<T: Serialize>(&self, run_id: &str, input: &T) -> Result<()> {
        let dir = self.run_dir(run_id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(self.input_path(run_id), serde_json::to_vec_pretty(input)?)
            .with_context(|| format!("write run input {}", run_id))?;
        Ok(())
    }

    pub fn load_input<T: for<'de> Deserialize<'de>>(&self, run_id: &str) -> Result<T> {
        let bytes = std::fs::read(self.input_path(run_id))
            .with_context(|| format!("read run input {}", run_id))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn append_event(&self, run_id: &str, event_type: &str, message: &str) -> Result<()> {
        let dir = self.run_dir(run_id);
        std::fs::create_dir_all(&dir)?;
        let _lock = AppendLock::acquire(self.lock_path(run_id))?;
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

    pub fn read_events(&self, run_id: &str) -> Result<Vec<RunEvent>> {
        let path = self.events_path(run_id);
        if !path.exists() {
            return Ok(vec![]);
        }
        read_json_lines(&path)
    }

    /// Append one durable mailbox event and assign its sequence/timestamp/id.
    pub fn append_mailbox_event(&self, mut event: MailboxEvent) -> Result<MailboxEvent> {
        let dir = self.run_dir(&event.run_id);
        std::fs::create_dir_all(&dir)?;
        let _lock = AppendLock::acquire(self.lock_path(&event.run_id))?;
        let path = self.mailbox_path(&event.run_id);
        event.sequence = self.reserve_next_mailbox_sequence(&event.run_id)?;
        event.timestamp_ms = now_ms();
        if event.event_id.is_empty() {
            event.event_id = format!("evt-{}", uuid::Uuid::new_v4());
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
        Ok(event)
    }

    pub fn read_mailbox(&self, run_id: &str) -> Result<Vec<MailboxEvent>> {
        let path = self.mailbox_path(run_id);
        if !path.exists() {
            return Ok(vec![]);
        }
        read_json_lines(&path)
    }

    pub fn read_mailbox_since(
        &self,
        run_id: &str,
        since_sequence: u64,
    ) -> Result<Vec<MailboxEvent>> {
        Ok(self
            .read_mailbox(run_id)?
            .into_iter()
            .filter(|event| event.sequence > since_sequence)
            .collect())
    }

    fn reserve_next_mailbox_sequence(&self, run_id: &str) -> Result<u64> {
        let seq_path = self.mailbox_seq_path(run_id);
        let current = match std::fs::read_to_string(&seq_path) {
            Ok(text) => text.trim().parse::<u64>().unwrap_or(0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self
                .read_mailbox(run_id)?
                .last()
                .map(|event| event.sequence)
                .unwrap_or(0),
            Err(e) => return Err(e).with_context(|| format!("read {}", seq_path.display())),
        };
        let next = current + 1;
        std::fs::write(&seq_path, format!("{}\n", next))
            .with_context(|| format!("write {}", seq_path.display()))?;
        Ok(next)
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

fn read_json_lines<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let text = std::fs::read_to_string(path)?;
    let mut items = vec![];
    for (idx, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let item = serde_json::from_str(line)
            .with_context(|| format!("parse JSON line {} in {}", idx + 1, path.display()))?;
        items.push(item);
    }
    Ok(items)
}

struct AppendLock {
    path: PathBuf,
}

impl AppendLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}:{}", std::process::id(), now_ms())?;
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&path)? {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        anyhow::bail!("timed out waiting for append lock {}", path.display());
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("create append lock {}", path.display()))
                }
            }
        }
    }
}

impl Drop for AppendLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path) -> Result<bool> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("stat lock {}", path.display())),
    };
    let Ok(modified) = meta.modified() else {
        return Ok(false);
    };
    Ok(modified
        .elapsed()
        .map(|age| age > LOCK_STALE_AFTER)
        .unwrap_or(false))
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

    #[test]
    fn mailbox_is_append_only_and_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::new(dir.path()).unwrap();
        let first = store
            .append_mailbox_event(
                MailboxEvent::new("run-1", "message", "hello")
                    .from("planner")
                    .to("builder")
                    .task_id("task-1")
                    .payload(serde_json::json!({ "checkpoint": "draft ready" })),
            )
            .unwrap();
        let second = store
            .append_mailbox_event(MailboxEvent::new("run-1", "sleep", "planner sleeping"))
            .unwrap();

        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert!(!first.event_id.is_empty());
        assert_eq!(
            std::fs::read_to_string(store.mailbox_seq_path("run-1"))
                .unwrap()
                .trim(),
            "2"
        );
        assert!(!store.lock_path("run-1").exists());

        let all = store.read_mailbox("run-1").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].from.as_deref(), Some("planner"));
        assert_eq!(all[0].to.as_deref(), Some("builder"));

        let since_first = store.read_mailbox_since("run-1", 1).unwrap();
        assert_eq!(since_first, vec![second]);
    }
}
