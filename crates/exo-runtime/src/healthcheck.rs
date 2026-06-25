//! Container healthcheck runner.
//!
//! Runs a configured probe command inside the container's namespaces on a
//! periodic interval and updates the container's persisted health status.

use crate::config::HealthcheckConfig;
use crate::events::{EventLog, EventType};
use crate::manager::ContainerManager;
use anyhow::Result;
use std::process::Stdio;
use std::time::Duration;

/// Current health state of a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HealthStatus {
    /// No healthcheck configured.
    #[default]
    None,
    /// Container has started; initial probes are running.
    Starting,
    /// Last probe succeeded.
    Healthy,
    /// Consecutive failures exceeded the configured retry count.
    Unhealthy,
}

impl HealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            HealthStatus::None => "none",
            HealthStatus::Starting => "starting",
            HealthStatus::Healthy => "healthy",
            HealthStatus::Unhealthy => "unhealthy",
        }
    }
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Background healthcheck worker for a single container.
pub struct HealthcheckRunner {
    container_name: String,
    container_pid: u32,
    config: HealthcheckConfig,
    status: HealthStatus,
    consecutive_failures: u32,
    started_at: std::time::Instant,
}

impl HealthcheckRunner {
    pub fn new(
        container_name: String,
        container_pid: u32,
        config: HealthcheckConfig,
    ) -> Self {
        Self {
            container_name,
            container_pid,
            config,
            status: HealthStatus::Starting,
            consecutive_failures: 0,
            started_at: std::time::Instant::now(),
        }
    }

    /// Run health probes until the container exits.
    /// This should be spawned as a background task so it does not block.
    pub async fn run(mut self) {
        let interval = Duration::from_secs(self.config.interval.max(1));
        let timeout = Duration::from_secs(self.config.timeout.max(1));
        let start_period = Duration::from_secs(self.config.start_period);

        // Initial delay so the container has time to start services.
        tokio::time::sleep(Duration::from_secs(1)).await;

        loop {
            // Stop probing if the container process is gone.
            if !is_process_alive(self.container_pid) {
                tracing::debug!(
                    "healthcheck: container {} pid {} is gone; stopping probe loop",
                    self.container_name,
                    self.container_pid
                );
                return;
            }

            let in_start_period = self.started_at.elapsed() < start_period;
            let probe_result = run_probe(self.container_pid, &self.config.test, timeout).await;

            match probe_result {
                Ok(true) => {
                    self.consecutive_failures = 0;
                    if self.status != HealthStatus::Healthy {
                        self.status = HealthStatus::Healthy;
                        self.persist_status();
                        self.emit_event(EventType::HealthcheckRecovered, None);
                    }
                }
                Ok(false) | Err(_) => {
                    // During the start period, failures keep us in Starting.
                    if in_start_period {
                        tracing::debug!(
                            "healthcheck: {} probe failed during start_period; ignoring",
                            self.container_name
                        );
                    } else {
                        self.consecutive_failures += 1;
                        let detail = probe_result.err().map(|e| e.to_string());
                        tracing::debug!(
                            "healthcheck: {} probe failed ({}/{})",
                            self.container_name,
                            self.consecutive_failures,
                            self.config.retries
                        );

                        if self.consecutive_failures >= self.config.retries
                            && self.status != HealthStatus::Unhealthy
                        {
                            self.status = HealthStatus::Unhealthy;
                            self.persist_status();
                            self.emit_event(EventType::HealthcheckFailed, detail.as_deref());
                        } else if self.status == HealthStatus::Starting
                            && self.consecutive_failures >= self.config.retries
                        {
                            // Transition from starting to unhealthy after retries.
                            self.status = HealthStatus::Unhealthy;
                            self.persist_status();
                            self.emit_event(EventType::HealthcheckFailed, detail.as_deref());
                        } else {
                            self.persist_status();
                        }
                    }
                }
            }

            tokio::time::sleep(interval).await;
        }
    }

    fn persist_status(&self) {
        let Ok(manager) = ContainerManager::new() else {
            return;
        };
        let Ok(Some(mut metadata)) = manager.find(&self.container_name) else {
            return;
        };
        metadata.health_status = Some(self.status.as_str().to_string());
        let _ = manager.save(&metadata);
    }

    fn emit_event(&self, event_type: EventType, detail: Option<&str>) {
        if let Ok(log) = EventLog::open_default() {
            let _ = log.record(
                &self.container_name,
                &self.container_name,
                event_type,
                detail,
            );
        }
    }
}

/// Check whether a process is still alive.
fn is_process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{}", pid)).exists()
}

/// Run a single probe command inside the container's namespaces.
/// Returns `Ok(true)` on success, `Ok(false)` on a clean probe failure, and
/// `Err` if the probe could not be executed.
async fn run_probe(pid: u32, test: &[String], timeout: Duration) -> Result<bool> {
    if test.is_empty() {
        return Ok(true);
    }

    // Prefer `nsenter` if available; it safely runs the command in the
    // container's mount/pid/network namespaces from a fresh process.
    let mut cmd = tokio::process::Command::new("nsenter");
    cmd.arg("--target")
        .arg(pid.to_string())
        .arg("--mount")
        .arg("--pid")
        .arg("--net")
        .arg("--")
        .args(test)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    match tokio::time::timeout(timeout, cmd.status()).await {
        Ok(Ok(status)) => Ok(status.success()),
        Ok(Err(e)) => {
            tracing::debug!("healthcheck probe failed to run: {}", e);
            Err(e.into())
        }
        Err(_) => {
            tracing::debug!("healthcheck probe timed out after {:?}", timeout);
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
        assert_eq!(HealthStatus::Starting.to_string(), "starting");
        assert_eq!(HealthStatus::None.to_string(), "none");
    }
}
