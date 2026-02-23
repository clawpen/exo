//! cgroups v2 resource management.

use anyhow::Result;
use std::path::PathBuf;

/// cgroups v2 manager for container resource limits.
pub struct CgroupManager {
    name: String,
    path: PathBuf,
}

impl CgroupManager {
    /// Create a new cgroup for a container.
    #[cfg(target_os = "linux")]
    pub fn new(name: &str) -> Result<Self> {
        let cgroup_path = PathBuf::from("/sys/fs/cgroup/openclaw");
        fs::create_dir_all(&cgroup_path)?;

        let container_path = cgroup_path.join(name);
        fs::create_dir_all(&container_path)?;

        tracing::debug!("Created cgroup: {}", container_path.display());

        Ok(Self {
            name: name.to_string(),
            path: container_path,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn new(name: &str) -> Result<Self> {
        tracing::warn!("Cgroups only supported on Linux");
        Ok(Self {
            name: name.to_string(),
            path: PathBuf::from("/sys/fs/cgroup/openclaw").join(name),
        })
    }

    /// Set memory limit in bytes.
    #[cfg(target_os = "linux")]
    pub fn set_memory_limit(&self, limit: u64) -> Result<()> {
        let memory_file = self.path.join("memory.max");
        fs::write(&memory_file, limit.to_string())?;
        tracing::debug!("Set memory limit: {} bytes", limit);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn set_memory_limit(&self, _limit: u64) -> Result<()> {
        Ok(())
    }

    /// Set CPU shares (relative weight).
    #[cfg(target_os = "linux")]
    pub fn set_cpu_shares(&self, shares: u64) -> Result<()> {
        let cpu_file = self.path.join("cpu.weight");
        // cgroups v2 uses 1-10000 scale, convert from 2-262
        let weight = (shares * 10000 / 1024).min(10000).max(1);
        fs::write(&cpu_file, weight.to_string())?;
        tracing::debug!("Set CPU shares: {}", weight);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn set_cpu_shares(&self, _shares: u64) -> Result<()> {
        Ok(())
    }

    /// Set CPU quota (e.g., "100000" for 1 CPU, "200000" for 2 CPUs).
    #[cfg(target_os = "linux")]
    pub fn set_cpu_quota(&self, quota_us: u64) -> Result<()> {
        let cpu_file = self.path.join("cpu.max");
        let period = 100000; // Default period
        fs::write(&cpu_file, format!("{} {}", quota_us, period))?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn set_cpu_quota(&self, _quota_us: u64) -> Result<()> {
        Ok(())
    }

    /// Set PIDs limit (max number of processes).
    #[cfg(target_os = "linux")]
    pub fn set_pids_limit(&self, limit: i64) -> Result<()> {
        let pids_file = self.path.join("pids.max");
        if limit > 0 {
            fs::write(&pids_file, limit.to_string())?;
        } else {
            fs::write(&pids_file, "max")?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn set_pids_limit(&self, _limit: i64) -> Result<()> {
        Ok(())
    }

    /// Add a process to this cgroup.
    #[cfg(target_os = "linux")]
    pub fn add_process(&self, pid: u32) -> Result<()> {
        let cgroup_file = self.path.join("cgroup.procs");
        fs::write(&cgroup_file, pid.to_string())?;
        tracing::debug!("Added process {} to cgroup {}", pid, self.name);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn add_process(&self, _pid: u32) -> Result<()> {
        Ok(())
    }

    /// Get current memory usage in bytes.
    #[cfg(target_os = "linux")]
    pub fn memory_usage(&self) -> Result<u64> {
        let memory_file = self.path.join("memory.current");
        let content = fs::read_to_string(&memory_file)?;
        Ok(content.trim().parse()?)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn memory_usage(&self) -> Result<u64> {
        Ok(0)
    }

    /// Get current CPU usage in nanoseconds.
    #[cfg(target_os = "linux")]
    pub fn cpu_usage(&self) -> Result<u64> {
        let cpu_file = self.path.join("cpu.stat");
        let content = fs::read_to_string(&cpu_file)?;

        // Parse "user <value>\nsystem <value>"
        for line in content.lines() {
            if let Some(value) = line.strip_prefix("user ") {
                return Ok(value.trim().parse()?);
            }
        }

        Ok(0)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn cpu_usage(&self) -> Result<u64> {
        Ok(0)
    }

    /// Delete the cgroup.
    #[cfg(target_os = "linux")]
    pub fn delete(&self) -> Result<()> {
        // Kill all processes in the cgroup first
        let kill_file = self.path.join("cgroup.kill");
        if kill_file.exists() {
            fs::write(&kill_file, "1")?;
        }

        // Remove the cgroup directory
        if self.path.exists() {
            fs::remove_dir(&self.path)?;
            tracing::debug!("Removed cgroup: {}", self.name);
        }

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn delete(&self) -> Result<()> {
        Ok(())
    }
}

impl Drop for CgroupManager {
    #[cfg(target_os = "linux")]
    fn drop(&mut self) {
        let _ = self.delete();
    }

    #[cfg(not(target_os = "linux"))]
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn test_cgroup_create() {
        // Only run if we have cgroups v2
        if PathBuf::from("/sys/fs/cgroup/cgroup.controllers").exists() {
            let cgroup = CgroupManager::new("test_container");
            assert!(cgroup.is_ok());

            let cgroup = cgroup.unwrap();
            assert!(cgroup.path.exists());

            // Clean up
            let _ = cgroup.delete();
        }
    }

    #[test]
    fn test_cgroup_new() {
        let cgroup = CgroupManager::new("test");
        assert!(cgroup.is_ok());
    }
}
