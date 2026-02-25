//! Cgroup v2 resource management for containers.
//!
//! This module provides cgroup v2 operations for limiting container resources
//! including memory, CPU, and process count. Cgroup v2 uses a unified hierarchy
//! at `/sys/fs/cgroup/`.
//!
//! # Cgroup v2 Hierarchy
//!
//! ```text
//! /sys/fs/cgroup/
//!   └── containment/
//!       └── <container_id>/
//!           ├── memory.max
//!           ├── cpu.max
//!           ├── pids.max
//!           ├── cgroup.procs
//!           └── cgroup.type
//! ```
//!
//! # Example
//!
//! ```no_run
//! use containment_runtime::cgroup::CgroupManager;
//!
//! let mut mgr = CgroupManager::new("my-container")?;
//! mgr.set_memory_limit(512 * 1024 * 1024)?;  // 512 MB
//! mgr.set_cpu_limit(100000, 100000)?;        // 1 CPU
//! mgr.set_pids_limit(100)?;
//! mgr.add_process(1234)?;
//! ```

use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use nix::unistd::Pid;

/// Cgroup v2 unified hierarchy mount point.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Containment cgroup subdirectory.
pub const CONTAINMENT_CGROUP: &str = "containment";

/// Default CPU quota period in microseconds (100ms).
pub const DEFAULT_CPU_PERIOD_US: u64 = 100_000;

/// Maximum value for pids.max (unlimited).
pub const PIDS_MAX: &str = "max";

/// Cgroup manager for container resource control.
///
/// Handles creation, configuration, and cleanup of cgroups for containers.
#[derive(Debug)]
pub struct CgroupManager {
    /// Container ID (used as cgroup name)
    container_id: String,

    /// Path to the container's cgroup directory
    cgroup_path: PathBuf,

    /// Whether the cgroup has been initialized
    initialized: bool,
}

impl CgroupManager {
    /// Create a new cgroup manager for a container.
    ///
    /// # Arguments
    ///
    /// * `container_id` - Unique identifier for the container
    ///
    /// # Returns
    ///
    /// A new CgroupManager instance
    pub fn new(container_id: &str) -> Result<Self> {
        let cgroup_path = PathBuf::from(CGROUP_ROOT)
            .join(CONTAINMENT_CGROUP)
            .join(container_id);

        Ok(Self {
            container_id: container_id.to_string(),
            cgroup_path,
            initialized: false,
        })
    }

    /// Initialize the cgroup by creating the directory.
    ///
    /// Creates the cgroup hierarchy: /sys/fs/cgroup/containment/<container_id>/
    pub fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        // Create the containment parent cgroup if it doesn't exist
        let parent_cgroup = PathBuf::from(CGROUP_ROOT).join(CONTAINMENT_CGROUP);
        if !parent_cgroup.exists() {
            fs::create_dir_all(&parent_cgroup)
                .with_context(|| format!("Failed to create cgroup parent: {:?}", parent_cgroup))?;

            // Enable all controllers for the subtree
            let subtree_control = parent_cgroup.join("cgroup.subtree_control");
            if subtree_control.exists() {
                let controllers = ["+cpu", "+memory", "+pids", "+io", "+cpuset"];
                for controller in &controllers {
                    // Write each controller
                    if let Err(e) = fs::write(&subtree_control, controller) {
                        tracing::debug!("Failed to enable controller {}: {}", controller, e);
                    }
                }
            }
        }

        // Create the container's cgroup directory
        fs::create_dir_all(&self.cgroup_path)
            .with_context(|| format!("Failed to create cgroup directory: {:?}", self.cgroup_path))?;

        self.initialized = true;

        tracing::debug!("Initialized cgroup at: {:?}", self.cgroup_path);

        Ok(())
    }

    /// Set memory limit for the container.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Maximum memory in bytes (use u64::MAX for unlimited)
    ///
    /// Writes to `memory.max` in the cgroup.
    pub fn set_memory_limit(&self, bytes: u64) -> Result<()> {
        self.ensure_initialized()?;

        let memory_max = self.cgroup_path.join("memory.max");

        let value = if bytes == u64::MAX {
            "max".to_string()
        } else {
            bytes.to_string()
        };

        fs::write(&memory_max, &value)
            .with_context(|| format!("Failed to write memory limit to {:?}", memory_max))?;

        tracing::debug!("Set memory limit: {} bytes", bytes);

        Ok(())
    }

    /// Set memory swap limit.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Maximum memory+swap in bytes (0 to disable swap)
    pub fn set_memory_swap_limit(&self, bytes: u64) -> Result<()> {
        self.ensure_initialized()?;

        let memory_swap_max = self.cgroup_path.join("memory.swap.max");

        let value = if bytes == 0 {
            "0".to_string()  // Disable swap
        } else if bytes == u64::MAX {
            "max".to_string()
        } else {
            bytes.to_string()
        };

        // This file may not exist if swap controller is not available
        if memory_swap_max.exists() {
            fs::write(&memory_swap_max, &value)
                .with_context(|| format!("Failed to write swap limit to {:?}", memory_swap_max))?;
            tracing::debug!("Set swap limit: {} bytes", bytes);
        } else {
            tracing::warn!("memory.swap.max not available");
        }

        Ok(())
    }

    /// Set CPU limit using quota and period.
    ///
    /// # Arguments
    ///
    /// * `quota_us` - CPU quota in microseconds (per period)
    /// * `period_us` - CPU period in microseconds (typically 100000)
    ///
    /// For example, to limit to 1 CPU: quota_us=100000, period_us=100000
    /// For 0.5 CPU: quota_us=50000, period_us=100000
    pub fn set_cpu_limit(&self, quota_us: u64, period_us: u64) -> Result<()> {
        self.ensure_initialized()?;

        let cpu_max = self.cgroup_path.join("cpu.max");

        let value = if quota_us == u64::MAX {
            format!("max {}", period_us)
        } else {
            format!("{} {}", quota_us, period_us)
        };

        fs::write(&cpu_max, &value)
            .with_context(|| format!("Failed to write CPU limit to {:?}", cpu_max))?;

        tracing::debug!("Set CPU limit: {} us / {} us", quota_us, period_us);

        Ok(())
    }

    /// Set CPU shares (relative weight).
    ///
    /// # Arguments
    ///
    /// * `shares` - CPU shares weight (1-10000, default 100)
    ///
    /// Higher values get more CPU time relative to other cgroups.
    pub fn set_cpu_shares(&self, shares: u64) -> Result<()> {
        self.ensure_initialized()?;

        let cpu_weight = self.cgroup_path.join("cpu.weight");

        // Map shares (1-10000) to weight (1-10000)
        let weight = shares.min(10000).max(1);

        fs::write(&cpu_weight, weight.to_string())
            .with_context(|| format!("Failed to write CPU weight to {:?}", cpu_weight))?;

        tracing::debug!("Set CPU weight: {}", weight);

        Ok(())
    }

    /// Set CPU affinity (which CPUs to use).
    ///
    /// # Arguments
    ///
    /// * `cpus` - CPU list (e.g., "0-3" or "0,2,4")
    pub fn set_cpu_affinity(&self, cpus: &str) -> Result<()> {
        self.ensure_initialized()?;

        let cpus_file = self.cgroup_path.join("cpuset.cpus");

        if cpus_file.exists() {
            // First need to copy parent's mems if cpuset is being used
            let parent_mems = PathBuf::from(CGROUP_ROOT)
                .join(CONTAINMENT_CGROUP)
                .join("cpuset.mems");

            if parent_mems.exists() {
                if let Ok(mems) = fs::read_to_string(&parent_mems) {
                    let mems_file = self.cgroup_path.join("cpuset.mems");
                    if mems_file.exists() {
                        let _ = fs::write(&mems_file, mems.trim());
                    }
                }
            }

            fs::write(&cpus_file, cpus)
                .with_context(|| format!("Failed to write CPU affinity to {:?}", cpus_file))?;

            tracing::debug!("Set CPU affinity: {}", cpus);
        } else {
            tracing::warn!("cpuset controller not available");
        }

        Ok(())
    }

    /// Set limit on number of processes.
    ///
    /// # Arguments
    ///
    /// * `max` - Maximum number of processes (0 or u64::MAX for unlimited)
    pub fn set_pids_limit(&self, max: u64) -> Result<()> {
        self.ensure_initialized()?;

        let pids_max = self.cgroup_path.join("pids.max");

        let value = if max == 0 || max == u64::MAX {
            PIDS_MAX
        } else {
            // Convert to string
            &max.to_string()
        };

        fs::write(&pids_max, value)
            .with_context(|| format!("Failed to write PIDs limit to {:?}", pids_max))?;

        tracing::debug!("Set PIDs limit: {}", value);

        Ok(())
    }

    /// Set I/O throttle limits.
    ///
    /// # Arguments
    ///
    /// * `dev` - Device major:minor (e.g., "259:0")
    /// * `read_bps` - Read bytes per second (0 for unlimited)
    /// * `write_bps` - Write bytes per second (0 for unlimited)
    pub fn set_io_throttle(&self, dev: &str, read_bps: u64, write_bps: u64) -> Result<()> {
        self.ensure_initialized()?;

        let io_max = self.cgroup_path.join("io.max");

        if io_max.exists() {
            let mut limits = Vec::new();

            if read_bps > 0 {
                limits.push(format!("{} rbps={}", dev, read_bps));
            }
            if write_bps > 0 {
                limits.push(format!("{} wbps={}", dev, write_bps));
            }

            if !limits.is_empty() {
                let value = limits.join(" ");
                fs::write(&io_max, &value)
                    .with_context(|| format!("Failed to write I/O limit to {:?}", io_max))?;

                tracing::debug!("Set I/O limits: {}", value);
            }
        }

        Ok(())
    }

    /// Add a process to the cgroup.
    ///
    /// # Arguments
    ///
    /// * `pid` - Process ID to add to the cgroup
    ///
    /// Writes to `cgroup.procs` in the cgroup.
    #[cfg(target_os = "linux")]
    pub fn add_process(&self, pid: Pid) -> Result<()> {
        self.ensure_initialized()?;

        let cgroup_procs = self.cgroup_path.join("cgroup.procs");

        let mut file = OpenOptions::new()
            .write(true)
            .open(&cgroup_procs)
            .with_context(|| format!("Failed to open cgroup.procs: {:?}", cgroup_procs))?;

        writeln!(file, "{}", pid.as_raw())
            .with_context(|| format!("Failed to write PID {} to cgroup.procs", pid.as_raw()))?;

        tracing::debug!("Added PID {} to cgroup {:?}", pid.as_raw(), self.cgroup_path);

        Ok(())
    }

    /// Add the current process to the cgroup.
    #[cfg(target_os = "linux")]
    pub fn add_current_process(&self) -> Result<()> {
        self.add_process(Pid::this())
    }

    /// Get the current memory usage in bytes.
    pub fn get_memory_usage(&self) -> Result<u64> {
        self.ensure_initialized()?;

        let memory_current = self.cgroup_path.join("memory.current");

        let content = fs::read_to_string(&memory_current)
            .with_context(|| format!("Failed to read memory.current from {:?}", memory_current))?;

        let usage = content.trim().parse::<u64>()
            .context("Failed to parse memory usage")?;

        Ok(usage)
    }

    /// Get the current memory limit in bytes.
    pub fn get_memory_limit(&self) -> Result<Option<u64>> {
        self.ensure_initialized()?;

        let memory_max = self.cgroup_path.join("memory.max");

        let content = fs::read_to_string(&memory_max)
            .with_context(|| format!("Failed to read memory.max from {:?}", memory_max))?;

        let trimmed = content.trim();

        if trimmed == "max" {
            Ok(None)
        } else {
            let limit = trimmed.parse::<u64>()
                .context("Failed to parse memory limit")?;
            Ok(Some(limit))
        }
    }

    /// Get the current CPU usage in nanoseconds.
    pub fn get_cpu_usage(&self) -> Result<u64> {
        self.ensure_initialized()?;

        let cpu_stat = self.cgroup_path.join("cpu.stat");

        let content = fs::read_to_string(&cpu_stat)
            .with_context(|| format!("Failed to read cpu.stat from {:?}", cpu_stat))?;

        // Parse "usage_usec" line and convert to nanoseconds
        for line in content.lines() {
            if line.starts_with("usage_usec ") {
                let us_str = line.trim_start_matches("usage_usec ");
                let us = us_str.parse::<u64>()
                    .context("Failed to parse CPU usage")?;
                return Ok(us * 1000); // Convert microseconds to nanoseconds
            }
        }

        Err(anyhow::anyhow!("usage_usec not found in cpu.stat"))
    }

    /// Get the list of PIDs in the cgroup.
    pub fn get_processes(&self) -> Result<Vec<i32>> {
        self.ensure_initialized()?;

        let cgroup_procs = self.cgroup_path.join("cgroup.procs");

        let content = fs::read_to_string(&cgroup_procs)
            .with_context(|| format!("Failed to read cgroup.procs from {:?}", cgroup_procs))?;

        let mut pids = Vec::new();
        for line in content.lines() {
            if let Ok(pid) = line.trim().parse::<i32>() {
                pids.push(pid);
            }
        }

        Ok(pids)
    }

    /// Check if cgroup v2 is available on the system.
    pub fn is_cgroup_v2() -> bool {
        let mountinfo = PathBuf::from("/proc/self/mountinfo");

        if let Ok(content) = fs::read_to_string(mountinfo) {
            for line in content.lines() {
                if line.contains("cgroup2") && line.contains(CGROUP_ROOT) {
                    return true;
                }
            }
        }

        false
    }

    /// Ensure the cgroup is initialized.
    fn ensure_initialized(&self) -> Result<()> {
        if !self.initialized {
            anyhow::bail!("Cgroup not initialized. Call initialize() first.");
        }

        if !self.cgroup_path.exists() {
            anyhow::bail!("Cgroup directory does not exist: {:?}", self.cgroup_path);
        }

        Ok(())
    }

    /// Destroy the cgroup and clean up resources.
    ///
    /// Removes the cgroup directory. All processes must have already
    /// exited the cgroup before this is called.
    pub fn destroy(mut self) -> Result<()> {
        if !self.initialized {
            return Ok(());
        }

        // Try to kill all processes in the cgroup
        let cgroup_procs = self.cgroup_path.join("cgroup.procs");
        if cgroup_procs.exists() {
            if let Ok(pids) = self.get_processes() {
                if !pids.is_empty() {
                    tracing::warn!("Cgroup still has processes: {:?}", pids);
                    // Try to terminate them
                    #[cfg(target_os = "linux")]
                    for pid in pids {
                        let _ = nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(pid),
                            nix::sys::signal::Signal::SIGKILL,
                        );
                    }

                    // Give them a moment to exit
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }

        // Remove the cgroup directory
        if self.cgroup_path.exists() {
            fs::remove_dir(&self.cgroup_path)
                .with_context(|| format!("Failed to remove cgroup directory: {:?}", self.cgroup_path))?;

            tracing::debug!("Destroyed cgroup: {:?}", self.cgroup_path);
        }

        self.initialized = false;

        Ok(())
    }

    /// Get the cgroup path.
    pub fn path(&self) -> &Path {
        &self.cgroup_path
    }

    /// Get the container ID.
    pub fn container_id(&self) -> &str {
        &self.container_id
    }
}

impl Drop for CgroupManager {
    fn drop(&mut self) {
        if self.initialized && self.cgroup_path.exists() {
            // Best-effort cleanup
            let _ = fs::remove_dir(&self.cgroup_path);
            self.initialized = false;
        }
    }
}

/// Parse a resource size string to bytes.
///
/// Supports: "512M", "2G", "1024K", "1B"
pub fn parse_size(size: &str) -> Result<u64> {
    let size = size.trim().to_uppercase();

    let (num_str, multiplier): (&str, u64) = if size.ends_with('B') {
        let s = &size[..size.len() - 1];
        if s.ends_with('G') {
            (&s[..s.len() - 1], 1024 * 1024 * 1024)
        } else if s.ends_with('M') {
            (&s[..s.len() - 1], 1024 * 1024)
        } else if s.ends_with('K') {
            (&s[..s.len() - 1], 1024)
        } else {
            (s, 1)
        }
    } else if size.ends_with('G') {
        (&size[..size.len() - 1], 1024 * 1024 * 1024)
    } else if size.ends_with('M') {
        (&size[..size.len() - 1], 1024 * 1024)
    } else if size.ends_with('K') {
        (&size[..size.len() - 1], 1024)
    } else {
        (&size[..], 1)
    };

    let num: u64 = num_str.parse()
        .context(format!("Invalid size format: {}", size))?;

    Ok(num.saturating_mul(multiplier))
}

/// Convert CPU count to quota/period values.
///
/// # Arguments
///
/// * `cpu_count` - Number of CPUs (e.g., 1.0 for 1 CPU, 0.5 for half a CPU)
///
/// # Returns
///
/// (quota_us, period_us) tuple
pub fn cpu_count_to_quota(cpu_count: f64) -> (u64, u64) {
    let period_us = DEFAULT_CPU_PERIOD_US;
    let quota_us = (cpu_count * period_us as f64) as u64;
    (quota_us, period_us)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1024K").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1B").unwrap(), 1);
    }

    #[test]
    fn test_cpu_count_to_quota() {
        let (quota, period) = cpu_count_to_quota(1.0);
        assert_eq!(period, 100_000);
        assert_eq!(quota, 100_000);

        let (quota, period) = cpu_count_to_quota(0.5);
        assert_eq!(period, 100_000);
        assert_eq!(quota, 50_000);

        let (quota, period) = cpu_count_to_quota(2.0);
        assert_eq!(period, 100_000);
        assert_eq!(quota, 200_000);
    }

    #[test]
    fn test_cgroup_manager_new() {
        let mgr = CgroupManager::new("test-container").unwrap();
        assert_eq!(mgr.container_id(), "test-container");
        assert_eq!(mgr.path(), PathBuf::from("/sys/fs/cgroup/containment/test-container"));
        assert!(!mgr.initialized);
    }
}
