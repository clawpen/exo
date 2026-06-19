//! Common test utilities for integration tests

use std::path::{Path, PathBuf};
use std::fs;
use std::process::Command;
use std::time::Duration;
use std::thread;
use anyhow::Result;
use tempfile::TempDir;

/// Test environment setup
pub struct TestEnv {
    pub temp_dir: TempDir,
    pub runtime_path: PathBuf,
    pub test_id: String,
}

impl TestEnv {
    pub fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let test_id = format!("test_{}", std::process::id());
        let runtime_path = temp_dir.path().join("runtime");
        fs::create_dir_all(&runtime_path)?;

        Ok(Self {
            temp_dir,
            runtime_path,
            test_id,
        })
    }

    pub fn storage_path(&self) -> PathBuf {
        self.temp_dir.path().join("storage")
    }

    pub fn cgroup_path(&self) -> PathBuf {
        PathBuf::from(format!("/sys/fs/cgroup/{}", self.test_id))
    }

    /// Create a minimal rootfs for testing
    pub fn create_minimal_rootfs(&self) -> Result<PathBuf> {
        let rootfs = self.temp_dir.path().join("rootfs");
        fs::create_dir_all(rootfs.join("bin"))?;
        fs::create_dir_all(rootfs.join("lib"))?;
        fs::create_dir_all(rootfs.join("lib64"))?;
        fs::create_dir_all(rootfs.join("etc"))?;
        fs::create_dir_all(rootfs.join("proc"))?;
        fs::create_dir_all(rootfs.join("dev"))?;
        fs::create_dir_all(rootfs.join("tmp"))?;

        // Create a minimal /etc/resolv.conf
        fs::write(rootfs.join("etc/resolv.conf"), "nameserver 8.8.8.8\n")?;

        Ok(rootfs)
    }

    /// Create a test rootfs with a simple test binary
    #[cfg(target_os = "linux")]
    pub fn create_test_rootfs_with_binary(&self) -> Result<PathBuf> {
        let rootfs = self.create_minimal_rootfs()?;

        // Copy /bin/sh to test rootfs if available
        if Path::new("/bin/sh").exists() {
            if let Ok(_) = fs::copy("/bin/sh", rootfs.join("bin/sh")) {
                // Try to copy dependencies
                Self::copy_dependencies(Path::new("/bin/sh"), &rootfs)?;
            }
        }

        // Create a simple test script
        let test_script = r#"#!/bin/sh
echo "Container is running"
while true; do
    sleep 1
done
"#;
        fs::write(rootfs.join("bin/test.sh"), test_script)?;

        Ok(rootfs)
    }

    #[cfg(target_os = "linux")]
    fn copy_dependencies(binary: &Path, rootfs: &Path) -> Result<()> {
        // Use ldd to find dependencies
        let output = Command::new("ldd")
            .arg(binary)
            .output()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Some(lib_path) = line.split(" => ").nth(1) {
                    let lib_path = lib_path.split_whitespace().next().unwrap_or("");
                    if !lib_path.is_empty() && Path::new(lib_path).exists() {
                        let dest = rootfs.join(lib_path.trim_start_matches('/'));
                        if let Some(parent) = dest.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let _ = fs::copy(lib_path, dest);
                    }
                }
            }
        }
        Ok(())
    }

    /// Clean up cgroup after test
    #[cfg(target_os = "linux")]
    pub fn cleanup_cgroup(&self) {
        let cgroup_path = self.cgroup_path();
        if cgroup_path.exists() {
            // Kill all processes in cgroup
            let procs_file = cgroup_path.join("cgroup.procs");
            if let Ok(content) = fs::read_to_string(&procs_file) {
                for pid_str in content.lines() {
                    if let Ok(pid) = pid_str.trim().parse::<u32>() {
                        let _ = Command::new("kill")
                            .arg("-9")
                            .arg(pid.to_string())
                            .output();
                    }
                }
            }
            // Delete subcgroups
            if let Ok(entries) = fs::read_dir(&cgroup_path) {
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        let _ = fs::remove_dir_all(entry.path());
                    }
                }
            }
            let _ = fs::remove_dir(&cgroup_path);
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        self.cleanup_cgroup();
    }
}

/// Wait for a condition with timeout
pub fn wait_for<F>(condition: F, timeout: Duration) -> Result<()>
where
    F: Fn() -> bool,
{
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("Timeout waiting for condition");
}

/// Assert file exists with timeout
pub fn assert_file_exists(path: &Path, timeout: Duration) -> Result<()> {
    wait_for(|| path.exists(), timeout)?;
    Ok(())
}

/// Assert process is running
pub fn assert_process_running(pid: u32, timeout: Duration) -> Result<bool> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if let Ok(_) = fs::metadata(format!("/proc/{}/status", pid)) {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(false)
}

/// Read process stat file
#[cfg(target_os = "linux")]
pub fn read_proc_stat(pid: u32) -> Result<String> {
    let stat_path = format!("/proc/{}/stat", pid);
    fs::read_to_string(stat_path).map_err(Into::into)
}

/// Check if cgroup v2 is available
#[cfg(target_os = "linux")]
pub fn is_cgroup_v2_available() -> bool {
    Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
}

/// Check if unprivileged user namespaces are available
#[cfg(target_os = "linux")]
pub fn is_userns_available() -> bool {
    // Try to create a simple user namespace
    
    use nix::unistd::getuid;

    if getuid().is_root() {
        return true;
    }

    // Check /proc/sys/user/max_user_namespaces
    if let Ok(max) = fs::read_to_string("/proc/sys/user/max_user_namespaces") {
        if let Ok(value) = max.trim().parse::<u32>() {
            return value > 0;
        }
    }

    false
}

/// Assert memory limit is enforced
#[cfg(target_os = "linux")]
pub fn assert_memory_limit(_pid: u32, expected_bytes: u64) -> Result<()> {
    thread::sleep(Duration::from_millis(500)); // Give cgroup time to apply

    let cgroup_memory_max = format!(
        "/sys/fs/cgroup/openclaw_test_{}/memory.max",
        std::process::id()
    );

    if Path::new(&cgroup_memory_max).exists() {
        let limit = fs::read_to_string(&cgroup_memory_max)?;
        let limit: u64 = limit.trim().parse()?;
        assert_eq!(limit, expected_bytes, "Memory limit mismatch");
    }

    Ok(())
}

/// Assert CPU limit is configured
#[cfg(target_os = "linux")]
pub fn assert_cpu_limit(_pid: u32, expected_quota: i64, expected_period: u64) -> Result<()> {
    thread::sleep(Duration::from_millis(500));

    let cgroup_path = format!("/sys/fs/cgroup/openclaw_test_{}", std::process::id());

    if let Ok(quota) = fs::read_to_string(format!("{}/cpu.max", cgroup_path)) {
        let parts: Vec<&str> = quota.trim().split_whitespace().collect();
        if parts.len() == 2 {
            let quota_val: i64 = parts[0].parse().unwrap_or(-1);
            let period: u64 = parts[1].parse().unwrap_or(100000);
            assert_eq!(quota_val, expected_quota, "CPU quota mismatch");
            assert_eq!(period, expected_period, "CPU period mismatch");
        }
    }

    Ok(())
}
