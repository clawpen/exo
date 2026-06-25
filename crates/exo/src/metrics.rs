//! Prometheus metrics exporter for the exo daemon.
//!
//! Exposes container-level and daemon-level metrics on an HTTP endpoint
//! (default `127.0.0.1:9090`). Metrics are sampled on a background interval
//! from the persisted container metadata and cgroup state.

use anyhow::{Context, Result};
use prometheus::{Encoder, Registry, IntGauge, Gauge, TextEncoder, register_int_gauge_with_registry, register_gauge_with_registry};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Default bind address for the metrics endpoint.
pub const DEFAULT_METRICS_ADDR: &str = "127.0.0.1:9090";

/// Prometheus metrics state.
pub struct DaemonMetrics {
    registry: Registry,
    /// Number of containers in each status.
    containers_running: IntGauge,
    containers_exited: IntGauge,
    containers_total: IntGauge,
    /// Aggregate memory usage across running containers (bytes).
    total_memory_bytes: Gauge,
    /// Per-container memory usage (bytes).
    container_memory_bytes: Gauge,
    /// Per-container CPU user time (seconds).
    container_cpu_seconds: Gauge,
    /// Per-container pids count.
    container_pids: IntGauge,
}

impl DaemonMetrics {
    pub fn new() -> Result<Self> {
        let registry = Registry::new();

        let containers_running = register_int_gauge_with_registry!(
            "exo_containers_running",
            "Number of containers currently running",
            registry
        )
        .context("Failed to register running container gauge")?;

        let containers_exited = register_int_gauge_with_registry!(
            "exo_containers_exited",
            "Number of containers currently exited",
            registry
        )
        .context("Failed to register exited container gauge")?;

        let containers_total = register_int_gauge_with_registry!(
            "exo_containers_total",
            "Total number of containers known to the daemon",
            registry
        )
        .context("Failed to register total container gauge")?;

        let total_memory_bytes = register_gauge_with_registry!(
            "exo_total_memory_bytes",
            "Aggregate memory usage of running containers",
            registry
        )
        .context("Failed to register total memory gauge")?;

        // Per-container gauges reuse the same metric family with a `container`
        // label set during collection.
        let container_memory_bytes = register_gauge_with_registry!(
            "exo_container_memory_bytes",
            "Memory usage of a container",
            registry
        )
        .context("Failed to register container memory gauge")?;

        let container_cpu_seconds = register_gauge_with_registry!(
            "exo_container_cpu_seconds",
            "CPU user time of a container",
            registry
        )
        .context("Failed to register container CPU gauge")?;

        let container_pids = register_int_gauge_with_registry!(
            "exo_container_pids",
            "Number of processes in a container cgroup",
            registry
        )
        .context("Failed to register container PIDs gauge")?;

        Ok(Self {
            registry,
            containers_running,
            containers_exited,
            containers_total,
            total_memory_bytes,
            container_memory_bytes,
            container_cpu_seconds,
            container_pids,
        })
    }

    /// Collect fresh metrics from the container manager.
    #[cfg(unix)]
    pub fn collect(&self, manager: &exo_runtime::ContainerManager,
    ) {
        let Ok(containers) = manager.list() else {
            return;
        };

        let mut running = 0i64;
        let mut exited = 0i64;
        let mut total_memory = 0.0f64;

        for container in containers {
            if container.is_running() {
                running += 1;
            } else if container.status == "exited" {
                exited += 1;
            }

            if !container.is_running() {
                continue;
            }

            // Cgroup stats via the runtime's CgroupManager.
            let name = container.name.clone();
            if let Ok(cg) = exo_runtime::CgroupManager::new(&container.config.name,
            ) {
                if let Ok(bytes) = cg.get_memory_usage() {
                    self.container_memory_bytes
                        .with_label_values(&[&name])
                        .set(bytes as f64);
                    total_memory += bytes as f64;
                }
                if let Ok(usecs) = cg.get_cpu_usage() {
                    // cgroup cpu.usage is typically in microseconds.
                    self.container_cpu_seconds
                        .with_label_values(&[&name])
                        .set(usecs as f64 / 1_000_000.0);
                }
                if let Ok(pids) = cg.get_processes() {
                    self.container_pids
                        .with_label_values(&[&name])
                        .set(pids.len() as i64);
                }
            }
        }

        self.containers_running.set(running);
        self.containers_exited.set(exited);
        self.containers_total.set((running + exited) as i64);
        self.total_memory_bytes.set(total_memory);
    }

    /// Non-Unix no-op.
    #[cfg(not(unix))]
    pub fn collect(&self, _manager: &exo_runtime::ContainerManager,
    ) {
    }

    /// Encode all metrics in Prometheus text format.
    pub fn encode(&self) -> Result<String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .context("Failed to encode metrics")?;
        String::from_utf8(buffer).context("Metrics output is not UTF-8")
    }
}

impl Default for DaemonMetrics {
    fn default() -> Self {
        Self::new().expect("Failed to create daemon metrics")
    }
}

/// Spawn the Prometheus metrics HTTP endpoint on a background thread.
pub fn spawn_server(addr: SocketAddr, metrics: Arc<DaemonMetrics>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let listener = match TcpListener::bind(addr) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Failed to bind metrics server to {}: {}", addr, e);
                return;
            }
        };
        tracing::info!("Prometheus metrics available at http://{}/metrics", addr);

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let metrics = metrics.clone();
                    thread::spawn(move || {
                        let _ = handle_request(stream, metrics);
                    });
                }
                Err(e) => {
                    tracing::debug!("Metrics server accept error: {}", e);
                }
            }
        }
    })
}

fn handle_request(
    mut stream: TcpStream,
    metrics: Arc<DaemonMetrics>,
) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone().context("Failed to clone stream")?);
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;

    // Drain headers.
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            break;
        }
    }

    let path = first_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/");

    let (status, body) = if path == "/metrics" {
        match metrics.encode() {
            Ok(text) => ("200 OK", text),
            Err(e) => {
                tracing::warn!("Failed to encode metrics: {}", e);
                ("500 Internal Server Error", "# error encoding metrics\n".to_string())
            }
        }
    } else {
        (
            "200 OK",
            "exo metrics server\nvisit /metrics for Prometheus exposition format\n".to_string(),
        )
    };

    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}

/// Spawn a background sampler that refreshes metrics on an interval.
pub fn spawn_sampler(
    manager: Arc<exo_runtime::ContainerManager>,
    metrics: Arc<DaemonMetrics>,
    interval: Duration,
) -> thread::JoinHandle<()> {
    thread::spawn(move || loop {
        thread::sleep(interval);
        metrics.collect(&manager);
    })
}
