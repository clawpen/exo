//! Reconciliation loop for the daemon.
//!
//! The reconciler closes the gap between *desired* state (what
//! `ContainerManager` says is running) and *actual* state (what `/proc` and
//! the cgroup hierarchy show). It is the **sole owner** of host-artifact GC:
//! the daemon's stop handler signals + marks metadata, but never touches
//! cgroups or overlays — the reconciler does, idempotently.
//!
//! Two entry points exist:
//!
//! - [`Reconciler::run_recovery_pass`] — called once at daemon startup,
//!   *before* accepting connections. Honors `RestartPolicy::OnDaemonRestart`.
//!
//! - [`Reconciler::run_loop`] — periodic background task. Does *not* honor
//!   restart policy: a container dying on its own merits should not be
//!   automatically restarted (only daemon-crash recovery should restart).
//!
//! Each per-container reconcile is independent + idempotent, so a daemon
//! crash mid-cycle still converges on the next pass.

use crate::config::RestartPolicy;
use crate::container::Container;
use crate::events::{EventLog, EventType};
use crate::manager::{ContainerManager, ContainerMetadata};
use anyhow::Result;
use chrono::Utc;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Default cgroup root for exo containers (matches `cgroup::CGROUP_ROOT` +
/// `cgroup::CONTAINMENT_CGROUP`).
pub const DEFAULT_CGROUP_ROOT: &str = "/sys/fs/cgroup/containment";

/// Configuration for the reconciler.
#[derive(Debug, Clone)]
pub struct ReconcileOptions {
    /// How often the periodic loop fires.
    pub interval: Duration,
    /// Where container cgroups live. Direct subdirs are container names.
    pub cgroup_root: PathBuf,
    /// If set, exited containers older than this are removed entirely
    /// (metadata + artifacts). `None` (default) keeps exited entries forever.
    pub stale_after: Option<Duration>,
}

impl Default for ReconcileOptions {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3),
            cgroup_root: PathBuf::from(DEFAULT_CGROUP_ROOT),
            stale_after: None,
        }
    }
}

/// Per-cycle outcome counts. Returned from [`Reconciler::run_once`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ReconcileSummary {
    /// Containers where actual state matched metadata.
    pub healthy: u32,
    /// Containers transitioned from "running" → "exited" this pass.
    pub exited: u32,
    /// Cgroups present without matching metadata. Killed + GC'd.
    pub orphaned: u32,
    /// Exited containers removed because they aged past `stale_after`.
    pub stale: u32,
    /// Containers respawned via `RestartPolicy::OnDaemonRestart`.
    pub restarted: u32,
    /// Operations that errored; details surface via tracing.
    pub errors: u32,
}

impl ReconcileSummary {
    fn merge(&mut self, other: ReconcileSummary) {
        self.healthy += other.healthy;
        self.exited += other.exited;
        self.orphaned += other.orphaned;
        self.stale += other.stale;
        self.restarted += other.restarted;
        self.errors += other.errors;
    }
}

/// The reconciler. Cheap to clone (everything internal is `Arc`).
#[derive(Clone)]
pub struct Reconciler {
    manager: Arc<ContainerManager>,
    events: Option<EventLog>,
    opts: ReconcileOptions,
    shutdown: Arc<tokio::sync::Notify>,
}

impl Reconciler {
    pub fn new(
        manager: Arc<ContainerManager>,
        events: Option<EventLog>,
        opts: ReconcileOptions,
    ) -> Self {
        Self {
            manager,
            events,
            opts,
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Synchronous reconcile pass. Suitable to call directly at daemon
    /// startup before the accept loop starts.
    ///
    /// `allow_restart` honors `RestartPolicy::OnDaemonRestart`. Set to true
    /// for the startup recovery pass; false for the periodic loop.
    pub fn run_once(&self, allow_restart: bool) -> ReconcileSummary {
        let mut summary = ReconcileSummary::default();

        // 1. Reconcile every container we know about.
        let containers = match self.manager.list() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("reconciler: failed to list containers: {}", e);
                summary.errors += 1;
                return summary;
            }
        };
        for metadata in containers {
            summary.merge(self.reconcile_one(metadata, allow_restart));
        }

        // 2. Sweep for cgroups without metadata (orphans).
        match self.detect_orphans() {
            Ok(n) => summary.orphaned += n,
            Err(e) => {
                tracing::warn!("reconciler: orphan sweep failed: {}", e);
                summary.errors += 1;
            }
        }

        summary
    }

    /// Run one synchronous recovery pass, restoring containers per
    /// `RestartPolicy::OnDaemonRestart`.
    pub fn run_recovery_pass(&self) -> ReconcileSummary {
        tracing::info!("reconciler: running startup recovery pass");
        let summary = self.run_once(true);
        tracing::info!(
            "reconciler: recovery pass complete: healthy={}, exited={}, orphaned={}, restarted={}, stale={}, errors={}",
            summary.healthy, summary.exited, summary.orphaned, summary.restarted, summary.stale, summary.errors
        );
        summary
    }

    /// Periodic loop. Honors shutdown signal; never restarts containers
    /// (process death after the daemon was already up means the container
    /// died on its own and shouldn't auto-recover under `OnDaemonRestart`).
    pub async fn run_loop(self) {
        tracing::info!(
            "reconciler: starting periodic loop (interval={:?})",
            self.opts.interval
        );
        let mut ticker = tokio::time::interval(self.opts.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let me = self.clone();
                    let summary = tokio::task::spawn_blocking(move || me.run_once(false))
                        .await
                        .unwrap_or_default();
                    if summary.exited + summary.orphaned + summary.stale + summary.errors > 0 {
                        tracing::debug!(
                            "reconciler tick: healthy={}, exited={}, orphaned={}, stale={}, errors={}",
                            summary.healthy, summary.exited, summary.orphaned, summary.stale, summary.errors
                        );
                    }
                }
                _ = self.shutdown.notified() => {
                    tracing::info!("reconciler: shutdown signal received");
                    return;
                }
            }
        }
    }

    /// Signal the periodic loop to stop on its next iteration.
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }

    fn reconcile_one(
        &self,
        mut metadata: ContainerMetadata,
        allow_restart: bool,
    ) -> ReconcileSummary {
        let mut summary = ReconcileSummary::default();

        // Already exited — only consider stale removal.
        if !metadata.is_running() {
            if let Some(after) = self.opts.stale_after {
                if let Some(stopped_at) = metadata.stopped_at {
                    let age = Utc::now().signed_duration_since(stopped_at);
                    if age.to_std().map(|d| d > after).unwrap_or(false) {
                        let name = metadata.name.clone();
                        let id = metadata.id.clone();
                        self.cleanup_full(&name);
                        if let Err(e) = self.manager.remove(&name) {
                            tracing::warn!("reconciler: failed to remove stale {}: {}", name, e);
                            summary.errors += 1;
                        } else {
                            self.emit(&id, &name, EventType::Gc, Some("stale removal"));
                            summary.stale += 1;
                            return summary;
                        }
                    }
                }
            }
            summary.healthy += 1;
            return summary;
        }

        // Metadata says running. Verify the process exists.
        let pid = metadata.pid.unwrap_or(0);
        if pid != 0 && proc_exists(pid) {
            // Healthy. Cgroup may or may not exist — log if missing but don't act.
            let cg = self.opts.cgroup_root.join(&metadata.name);
            if !cg.exists() {
                tracing::warn!(
                    "reconciler: {} pid {} alive but cgroup {:?} missing",
                    metadata.name,
                    pid,
                    cg
                );
            }
            summary.healthy += 1;
            return summary;
        }

        // Process gone. Mark exited, GC artifacts (keep overlay so restart
        // can reuse it; only `stale` removes overlays).
        let name = metadata.name.clone();
        let id = metadata.id.clone();
        let policy = metadata.config.restart_policy;

        metadata.set_stopped(None);
        if let Err(e) = self.manager.save(&metadata) {
            tracing::warn!("reconciler: failed to mark {} exited: {}", name, e);
            summary.errors += 1;
            return summary;
        }
        self.cleanup_cgroup(&name);
        self.emit(&id, &name, EventType::Exited, Some("reconciler-detected"));
        summary.exited += 1;

        // Restart if recovery pass + policy allows.
        if allow_restart && policy == RestartPolicy::OnDaemonRestart {
            match self.restart(&mut metadata) {
                Ok(()) => {
                    if let Err(e) = self.manager.save(&metadata) {
                        tracing::warn!(
                            "reconciler: restart of {} succeeded but save failed: {}",
                            name,
                            e
                        );
                        summary.errors += 1;
                    } else {
                        self.emit(&metadata.id, &name, EventType::Restarted, None);
                        summary.restarted += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!("reconciler: restart of {} failed: {}", name, e);
                    summary.errors += 1;
                }
            }
        }

        summary
    }

    fn detect_orphans(&self) -> Result<u32> {
        if !self.opts.cgroup_root.exists() {
            return Ok(0);
        }

        let known: HashSet<String> = self.manager.list()?.into_iter().map(|m| m.name).collect();

        let mut count: u32 = 0;
        for entry in std::fs::read_dir(&self.opts.cgroup_root)? {
            let entry = entry?;
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if known.contains(&name) {
                continue;
            }

            // Orphan. Kill any pids in the cgroup, then remove.
            self.kill_cgroup_pids(&entry.path());
            // Remove the dir last; ignore errors (might be busy).
            if let Err(e) = std::fs::remove_dir(entry.path()) {
                tracing::debug!("reconciler: orphan cgroup {} remove failed: {}", name, e);
            }
            self.emit(&name, &name, EventType::ReconcileOrphan, None);
            count += 1;
        }

        Ok(count)
    }

    fn restart(&self, metadata: &mut ContainerMetadata) -> Result<()> {
        // Build a fresh Container from the saved config; spawn it.
        let mut container = Container::new(metadata.config.clone())?;
        container.start()?;

        // Container::new generated a new UUID. Capture it + new pid.
        let new_id = container.handle().id.clone();
        let new_pid = container.handle().pid.unwrap_or(0);
        metadata.id = new_id;
        if new_pid != 0 {
            metadata.set_running(new_pid);
        }
        Ok(())
    }

    fn kill_cgroup_pids(&self, cgroup_dir: &Path) {
        let procs = cgroup_dir.join("cgroup.procs");
        let Ok(content) = std::fs::read_to_string(&procs) else {
            return;
        };
        for line in content.lines() {
            let Ok(pid) = line.trim().parse::<i32>() else {
                continue;
            };
            // SIGKILL — orphans get no grace period.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }

    fn cleanup_cgroup(&self, name: &str) {
        let dir = self.opts.cgroup_root.join(name);
        if dir.exists() {
            // Only succeeds if cgroup has no procs left. That's expected
            // because the process is gone (we got here from exit detection)
            // or we just SIGKILL'd everything (orphan path).
            let _ = std::fs::remove_dir(&dir);
        }
    }

    fn cleanup_full(&self, name: &str) {
        self.cleanup_cgroup(name);
        // Overlay upper/work/rootfs/fs/config dirs live under the manager's
        // state_dir at `<state_dir>/<name>/`. The caller invokes
        // `self.manager.remove(name)` immediately after, which does
        // `fs::remove_dir_all(<state_dir>/<name>)` — that removes all overlay
        // artifacts. So nothing extra to do here unless container state is
        // ever stored outside the manager's state_dir (it currently isn't).
    }

    fn emit(&self, container_id: &str, container_name: &str, ty: EventType, detail: Option<&str>) {
        if let Some(events) = &self.events {
            if let Err(e) = events.record(container_id, container_name, ty, detail) {
                tracing::warn!("reconciler: failed to record event {:?}: {}", ty, e);
            }
        }
    }
}

fn proc_exists(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        return Path::new(&format!("/proc/{}", pid)).exists();
    }

    #[cfg(not(target_os = "linux"))]
    {
        let rc = unsafe { libc::kill(pid as i32, 0) };
        if rc == 0 {
            return true;
        }

        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ContainerConfig;
    use tempfile::TempDir;

    fn make_reconciler() -> (Reconciler, TempDir, TempDir) {
        let state_dir = TempDir::new().unwrap();
        let cgroup_dir = TempDir::new().unwrap();
        let manager = Arc::new(ContainerManager::with_state_dir(state_dir.path()).unwrap());
        let events_dir = TempDir::new().unwrap();
        let events = EventLog::open(events_dir.path().join("e.db")).unwrap();
        let opts = ReconcileOptions {
            interval: Duration::from_secs(1),
            cgroup_root: cgroup_dir.path().to_path_buf(),
            stale_after: None,
        };
        let reconciler = Reconciler::new(manager, Some(events), opts);
        // Keep events_dir alive by leaking it — alternatively return it.
        std::mem::forget(events_dir);
        (reconciler, state_dir, cgroup_dir)
    }

    fn make_metadata(name: &str, pid: u32) -> ContainerMetadata {
        let config = ContainerConfig {
            name: name.to_string(),
            image: "alpine:latest".to_string(),
            command: vec!["sleep".to_string(), "60".to_string()],
            ..Default::default()
        };
        let mut m = ContainerMetadata::new(name.to_string(), config);
        m.set_running(pid);
        m
    }

    #[test]
    fn test_healthy_when_pid_alive() {
        let (rec, _state, _cg) = make_reconciler();
        // Use our own pid — guaranteed alive.
        let pid = std::process::id();
        let m = make_metadata("self", pid);
        rec.manager.save(&m).unwrap();

        let summary = rec.run_once(false);
        assert_eq!(summary.healthy, 1);
        assert_eq!(summary.exited, 0);
        assert_eq!(summary.errors, 0);
    }

    #[test]
    fn test_exit_detection_when_pid_gone() {
        let (rec, _state, _cg) = make_reconciler();
        // Use a definitely-dead pid (process id 0 reserved on Linux for
        // scheduler; pick a high one unlikely to exist).
        let m = make_metadata("ghost", 999_999);
        rec.manager.save(&m).unwrap();

        let summary = rec.run_once(false);
        assert_eq!(summary.exited, 1);
        assert_eq!(summary.healthy, 0);

        // Metadata should now reflect exited.
        let after = rec.manager.load("ghost").unwrap();
        assert!(!after.is_running());
    }

    #[test]
    fn test_no_restart_when_periodic() {
        // OnDaemonRestart policy + periodic loop = no restart.
        let (rec, _state, _cg) = make_reconciler();
        let mut m = make_metadata("policy-test", 999_999);
        m.config.restart_policy = RestartPolicy::OnDaemonRestart;
        rec.manager.save(&m).unwrap();

        let summary = rec.run_once(false); // false = periodic, no restart
        assert_eq!(summary.exited, 1);
        assert_eq!(summary.restarted, 0);
    }

    #[test]
    fn test_orphan_cgroup_removed() {
        let (rec, _state, cg) = make_reconciler();
        // Create a fake orphan cgroup directory (no metadata for it).
        // We do *not* write a cgroup.procs file: real kernel cgroup
        // virtual files vanish on rmdir, so the test mirrors that by
        // leaving the dir empty. (kill_cgroup_pids handles missing
        // procs files silently.)
        let orphan = cg.path().join("rogue");
        std::fs::create_dir(&orphan).unwrap();

        let summary = rec.run_once(false);
        assert_eq!(summary.orphaned, 1);
        assert!(!orphan.exists(), "orphan cgroup dir should be removed");
    }

    #[test]
    fn test_stale_removal_respects_age() {
        let state_dir = TempDir::new().unwrap();
        let cgroup_dir = TempDir::new().unwrap();
        let manager = Arc::new(ContainerManager::with_state_dir(state_dir.path()).unwrap());
        let opts = ReconcileOptions {
            interval: Duration::from_secs(1),
            cgroup_root: cgroup_dir.path().to_path_buf(),
            stale_after: Some(Duration::from_millis(1)), // basically anything past now
        };
        let rec = Reconciler::new(manager.clone(), None, opts);

        let mut m = make_metadata("old-exited", 1);
        m.set_stopped(Some(0));
        // Backdate stopped_at well past stale_after.
        m.stopped_at = Some(Utc::now() - chrono::Duration::seconds(60));
        manager.save(&m).unwrap();

        let summary = rec.run_once(false);
        assert_eq!(summary.stale, 1);
        assert!(!manager.exists("old-exited"));
    }

    #[test]
    fn test_stale_removal_gcs_overlay_artifacts() {
        // Stale removal should rm -rf the entire {state_dir}/<name>/ tree,
        // including overlay upper/, work/, rootfs/, fs/, and config/.
        let state_dir = TempDir::new().unwrap();
        let cgroup_dir = TempDir::new().unwrap();
        let manager = Arc::new(ContainerManager::with_state_dir(state_dir.path()).unwrap());
        let opts = ReconcileOptions {
            interval: Duration::from_secs(1),
            cgroup_root: cgroup_dir.path().to_path_buf(),
            stale_after: Some(Duration::from_millis(1)),
        };
        let rec = Reconciler::new(manager.clone(), None, opts);

        // Create a stale exited container with overlay artifacts on disk.
        let mut m = make_metadata("agent-with-overlay", 1);
        m.set_stopped(Some(0));
        m.stopped_at = Some(Utc::now() - chrono::Duration::seconds(60));
        manager.save(&m).unwrap();

        // Fake the overlay/fs/config/upper/work/rootfs subdirs the runtime
        // would have created. Each holds a sentinel file we can grep for.
        let container_dir = state_dir.path().join("agent-with-overlay");
        for sub in ["upper", "work", "rootfs", "fs", "config"] {
            let p = container_dir.join(sub);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("sentinel"), b"x").unwrap();
        }
        // Pre-conditions.
        assert!(container_dir.join("upper/sentinel").exists());
        assert!(container_dir.join("rootfs/sentinel").exists());

        let summary = rec.run_once(false);
        assert_eq!(summary.stale, 1);

        // Post-conditions: every overlay subtree is gone.
        assert!(!container_dir.exists(), "container dir should be removed");
        assert!(!manager.exists("agent-with-overlay"));
    }

    #[test]
    fn test_keeps_recently_exited() {
        let state_dir = TempDir::new().unwrap();
        let cgroup_dir = TempDir::new().unwrap();
        let manager = Arc::new(ContainerManager::with_state_dir(state_dir.path()).unwrap());
        let opts = ReconcileOptions {
            interval: Duration::from_secs(1),
            cgroup_root: cgroup_dir.path().to_path_buf(),
            stale_after: Some(Duration::from_secs(3600)),
        };
        let rec = Reconciler::new(manager.clone(), None, opts);

        let mut m = make_metadata("just-exited", 1);
        m.set_stopped(Some(0)); // stopped_at = now
        manager.save(&m).unwrap();

        let summary = rec.run_once(false);
        assert_eq!(summary.stale, 0);
        assert_eq!(summary.healthy, 1);
        assert!(manager.exists("just-exited"));
    }
}
