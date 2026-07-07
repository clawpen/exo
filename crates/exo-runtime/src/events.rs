//! Lifecycle event log.
//!
//! Ring-buffered (~10k rows) sqlite log of container lifecycle transitions:
//! created/started/exited/killed/gc/reconcile_orphan/restarted.
//!
//! Used by the daemon and reconciler for post-hoc debugging at scale — when
//! something goes wrong with one of N=1000 containers, `tracing` logs are
//! gone after a daemon restart but this table persists.
//!
//! The log is fire-and-forget from the caller's perspective: failures are
//! returned as `Result` so tests can assert, but production callers should
//! discard or warn-log the error and continue (a missed event is never
//! worth failing a container operation).

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum rows retained. Older rows are trimmed after each insert.
const RING_BUFFER_SIZE: i64 = 10_000;

/// Default event log path under the system state dir.
pub const EVENT_LOG_PATH: &str = "/var/lib/exo/events.db";

/// Fallback when system path isn't writable (matches ContainerManager fallback).
pub const FALLBACK_EVENT_LOG_PATH: &str = ".local/share/exo/events.db";

/// A single recorded lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts_millis: i64,
    pub container_id: String,
    pub container_name: String,
    pub event_type: EventType,
    pub detail: Option<String>,
}

/// Lifecycle event categories. Values are stable on-disk strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Admission accepted; we are about to attempt Container::new.
    Created,
    /// Container::start succeeded; pid recorded in detail.
    Started,
    /// Anything between Created and Started failed (Container::new error,
    /// Container::start error, metadata save error). Detail carries the cause.
    Failed,
    StopRequested,
    Killed,
    Exited,
    Gc,
    ReconcileOrphan,
    Restarted,
}

impl EventType {
    fn as_str(self) -> &'static str {
        match self {
            EventType::Created => "created",
            EventType::Started => "started",
            EventType::Failed => "failed",
            EventType::StopRequested => "stop_requested",
            EventType::Killed => "killed",
            EventType::Exited => "exited",
            EventType::Gc => "gc",
            EventType::ReconcileOrphan => "reconcile_orphan",
            EventType::Restarted => "restarted",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "created" => EventType::Created,
            "started" => EventType::Started,
            "failed" => EventType::Failed,
            "stop_requested" => EventType::StopRequested,
            "killed" => EventType::Killed,
            "exited" => EventType::Exited,
            "gc" => EventType::Gc,
            "reconcile_orphan" => EventType::ReconcileOrphan,
            "restarted" => EventType::Restarted,
            _ => return None,
        })
    }
}

/// Cheap-to-clone handle over a single shared sqlite connection.
#[derive(Clone)]
pub struct EventLog {
    inner: Arc<EventLogInner>,
}

struct EventLogInner {
    conn: Mutex<Connection>,
}

impl EventLog {
    /// Open or create the event log at an explicit path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create event log dir {:?}", parent))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open event log at {:?}", path))?;

        // WAL gives us concurrent readers + one writer; NORMAL sync is enough
        // for an audit log (we don't need fsync-per-commit durability).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                container_id TEXT NOT NULL,
                container_name TEXT NOT NULL,
                event_type TEXT NOT NULL,
                detail TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts);
            CREATE INDEX IF NOT EXISTS idx_events_container ON events(container_id);",
        )?;

        Ok(Self {
            inner: Arc::new(EventLogInner {
                conn: Mutex::new(conn),
            }),
        })
    }

    /// Open at the default system path, falling back to the user-local path.
    pub fn open_default() -> Result<Self> {
        if let Ok(log) = Self::open(EVENT_LOG_PATH) {
            return Ok(log);
        }

        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let fallback = PathBuf::from(home).join(FALLBACK_EVENT_LOG_PATH);
        Self::open(fallback)
    }

    /// Record a lifecycle event. Fire-and-forget: callers should generally
    /// `let _ = log.record(...)` or warn-log on failure.
    pub fn record(
        &self,
        container_id: &str,
        container_name: &str,
        event_type: EventType,
        detail: Option<&str>,
    ) -> Result<()> {
        let conn = self.inner.conn.lock().expect("event log mutex poisoned");
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        conn.execute(
            "INSERT INTO events (ts, container_id, container_name, event_type, detail)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                ts,
                container_id,
                container_name,
                event_type.as_str(),
                detail
            ],
        )?;

        // Trim ring buffer. Bounded by the PK index, so O(deleted_rows).
        // Most inserts find nothing to delete and return immediately.
        conn.execute(
            "DELETE FROM events WHERE id <= COALESCE((SELECT MAX(id) FROM events), 0) - ?1",
            params![RING_BUFFER_SIZE],
        )?;

        Ok(())
    }

    /// Most recent N events (newest first).
    pub fn recent(&self, limit: usize) -> Result<Vec<Event>> {
        let conn = self.inner.conn.lock().expect("event log mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT ts, container_id, container_name, event_type, detail
             FROM events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let event_type: String = row.get(3)?;
            Ok(Event {
                ts_millis: row.get(0)?,
                container_id: row.get(1)?,
                container_name: row.get(2)?,
                event_type: EventType::from_str(&event_type).unwrap_or(EventType::Created),
                detail: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Most recent N events for a single container (newest first).
    pub fn for_container(&self, container_id: &str, limit: usize) -> Result<Vec<Event>> {
        let conn = self.inner.conn.lock().expect("event log mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT ts, container_id, container_name, event_type, detail
             FROM events WHERE container_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![container_id, limit as i64], |row| {
            let event_type: String = row.get(3)?;
            Ok(Event {
                ts_millis: row.get(0)?,
                container_id: row.get(1)?,
                container_name: row.get(2)?,
                event_type: EventType::from_str(&event_type).unwrap_or(EventType::Created),
                detail: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Total rows currently stored. Mainly for tests / debugging.
    pub fn count(&self) -> Result<i64> {
        let conn = self.inner.conn.lock().expect("event log mutex poisoned");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_log() -> (EventLog, TempDir) {
        let dir = TempDir::new().unwrap();
        let log = EventLog::open(dir.path().join("events.db")).unwrap();
        (log, dir)
    }

    #[test]
    fn test_record_and_recent() {
        let (log, _dir) = make_log();
        log.record("id1", "c1", EventType::Created, None).unwrap();
        log.record("id1", "c1", EventType::Started, Some("pid=1234"))
            .unwrap();

        let recent = log.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].event_type, EventType::Started);
        assert_eq!(recent[0].detail.as_deref(), Some("pid=1234"));
        assert_eq!(recent[1].event_type, EventType::Created);
        assert!(recent[0].ts_millis >= recent[1].ts_millis);
    }

    #[test]
    fn test_for_container_filter() {
        let (log, _dir) = make_log();
        log.record("a", "ca", EventType::Created, None).unwrap();
        log.record("b", "cb", EventType::Created, None).unwrap();
        log.record("a", "ca", EventType::Started, None).unwrap();

        let only_a = log.for_container("a", 10).unwrap();
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|e| e.container_id == "a"));
    }

    #[test]
    fn test_ring_buffer_trims() {
        // Insert RING_BUFFER_SIZE + 50 events; verify count caps and oldest are gone.
        let (log, _dir) = make_log();
        let total = (RING_BUFFER_SIZE + 50) as usize;
        for i in 0..total {
            log.record("c", "name", EventType::Created, Some(&i.to_string()))
                .unwrap();
        }
        let count = log.count().unwrap();
        assert!(count <= RING_BUFFER_SIZE, "count {} exceeded cap", count);

        // The oldest detail value (0) must be gone.
        let recent = log.recent(RING_BUFFER_SIZE as usize).unwrap();
        assert!(!recent.iter().any(|e| e.detail.as_deref() == Some("0")));
        // The newest detail value must still be there.
        assert!(recent
            .iter()
            .any(|e| e.detail.as_deref() == Some(&(total - 1).to_string())));
    }

    #[test]
    fn test_event_type_roundtrip() {
        let cases = [
            EventType::Created,
            EventType::Started,
            EventType::Failed,
            EventType::StopRequested,
            EventType::Killed,
            EventType::Exited,
            EventType::Gc,
            EventType::ReconcileOrphan,
            EventType::Restarted,
        ];
        for et in cases {
            assert_eq!(EventType::from_str(et.as_str()), Some(et));
        }
    }

    #[test]
    fn test_clone_handle_shares_db() {
        let (log, _dir) = make_log();
        let log2 = log.clone();
        log.record("c", "n", EventType::Created, None).unwrap();
        let visible = log2.recent(10).unwrap();
        assert_eq!(visible.len(), 1);
    }
}
