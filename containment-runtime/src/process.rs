//! Container process management using fork/exec with namespaces.

use crate::container::ContainerProcessConfig;
use anyhow::Result;
use std::ffi::CString;

/// Container process handle.
pub struct ContainerProcess {
    #[cfg(target_os = "linux")]
    pid: nix::unistd::Pid,
    #[cfg(not(target_os = "linux"))]
    pid: u32,
    log_file: Option<String>,
}

impl ContainerProcess {
    /// Spawn a new container process.
    #[cfg(target_os = "linux")]
    pub fn spawn(config: ContainerProcessConfig) -> Result<Self> {
        use nix::sys::wait::{waitpid, WaitStatus};
        use nix::unistd::{fork, ForkResult};
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        // Fork to create container process
        match unsafe { fork()? } {
            ForkResult::Parent { child } => {
                tracing::info!("Container process spawned with PID: {}", child);
                Ok(Self {
                    pid: child,
                    log_file: Some(format!("/var/lib/openclaw/containers/{}/logs/container.log", config.container_id)),
                })
            }
            ForkResult::Child => {
                // We're in the child process - set up container environment
                if let Err(e) = Self::child_setup(config) {
                    eprintln!("Container setup failed: {}", e);
                    std::process::exit(1);
                }
                // Should not reach here
                std::process::exit(1);
            }
        }
    }

    /// Spawn stub for non-Linux platforms.
    #[cfg(not(target_os = "linux"))]
    pub fn spawn(config: ContainerProcessConfig) -> Result<Self> {
        tracing::warn!("Container process spawning only supported on Linux");
        // Return a fake process handle
        Ok(Self {
            pid: std::process::id(),
            log_file: Some(format!("/var/lib/openclaw/containers/{}/logs/container.log", config.container_id)),
        })
    }

    /// Set up the container environment (runs in child process after fork).
    #[cfg(target_os = "linux")]
    fn child_setup(config: ContainerProcessConfig) -> Result<()> {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;
        use std::path::Path;

        // 1. Create new namespaces
        let namespaces = vec![
            Namespace::Mount,
            Namespace::Uts,
            Namespace::Ipc,
            Namespace::Pid,
            Namespace::Network,
        ];
        unshare_namespaces(&namespaces)?;

        // 2. Set hostname
        set_hostname(&format!("openclaw-{}", &config.container_id[..8]))?;

        // 3. Set up rootfs and mounts
        let mount_setup = MountSetup::new(&config.rootfs);
        mount_setup.setup_mounts(&config.mounts)?;

        // 4. Pivot/chroot to new root
        mount_setup.chroot_root()?;

        // 5. Set up cgroup for this process
        if let Ok(mut cgroup) = CgroupManager::new(&config.container_id) {
            // Add ourselves to the cgroup
            let _ = cgroup.add_process(std::process::id() as u32);
        }

        // 6. Change to working directory
        std::env::set_current_dir(&config.workdir)?;

        // 7. Set up environment variables
        for env_var in &config.env {
            if let Some((key, value)) = env_var.split_once('=') {
                std::env::set_var(key, value);
            }
        }

        // Add default environment
        std::env::set_var("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
        std::env::set_var("TERM", "xterm");
        std::env::set_var("HOME", "/root");

        // 8. Set up GPU environment if requested
        if config.gpu {
            std::env::set_var("NVIDIA_VISIBLE_DEVICES", "all");
            std::env::set_var("NVIDIA_DRIVER_CAPABILITIES", "compute,utility");
        }

        // 9. Open log file
        let log_path = format!("/var/lib/openclaw/containers/{}/logs/container.log", config.container_id);
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&log_path)
        {
            // Redirect stdout and stderr to log file
            if let Ok(fd) = file.as_raw_fd().try_into() {
                let _ = nix::unistd::dup2(fd, libc::STDOUT_FILENO);
                let _ = nix::unistd::dup2(fd, libc::STDERR_FILENO);
            }
        }

        // 10. Execute the container command
        Self::exec_command(&config.command)?;

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn child_setup(_config: ContainerProcessConfig) -> Result<()> {
        Err(anyhow::anyhow!("Container setup only supported on Linux"))
    }

    #[cfg(target_os = "linux")]
    fn exec_command(command: &[String]) -> Result<()> {
        if command.is_empty() {
            return Err(anyhow::anyhow!("Empty command"));
        }

        let program = &command[0];
        let args: Vec<CString> = command
            .iter()
            .map(|s| CString::new(s.as_bytes()).unwrap())
            .collect();

        tracing::info!("Executing: {:?}", command);

        // Use execve
        nix::unistd::execve(&CString::new(program.as_bytes())?, &args, &Self::get_env_vars())?;

        // Should not reach here
        Err(anyhow::anyhow!("exec failed"))
    }

    #[cfg(not(target_os = "linux"))]
    fn exec_command(_command: &[String]) -> Result<()> {
        Err(anyhow::anyhow!("exec only supported on Linux"))
    }

    #[cfg(target_os = "linux")]
    fn get_env_vars() -> Vec<CString> {
        std::env::vars()
            .map(|(k, v)| CString::new(format!("{}={}", k, v)).unwrap())
            .collect()
    }

    #[cfg(not(target_os = "linux"))]
    fn get_env_vars() -> Vec<CString> {
        vec![]
    }

    /// Get the process ID.
    #[cfg(target_os = "linux")]
    pub fn pid(&self) -> nix::unistd::Pid {
        self.pid
    }

    #[cfg(not(target_os = "linux"))]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Wait for the process to exit.
    #[cfg(target_os = "linux")]
    pub fn wait(&self) -> Result<i32> {
        use nix::sys::wait::{waitpid, WaitStatus};
        use nix::errno::Errno;

        match waitpid(self.pid, None) {
            Ok(WaitStatus::Exited(_, code)) => Ok(code),
            Ok(WaitStatus::Signaled(_, signal, _)) => Ok(128 + signal as i32),
            Ok(_) => Ok(0),
            Err(Errno::ESRCH) => {
                // Process already exited and was reaped - treat as success
                tracing::debug!("Process {} already reaped, assuming exit 0", self.pid);
                Ok(0)
            }
            Err(e) => Err(e.into()),
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn wait(&self) -> Result<i32> {
        Ok(0)
    }

    /// Check if the process is still running.
    #[cfg(target_os = "linux")]
    pub fn is_running(&self) -> bool {
        // Send signal 0 to check if process exists
        nix::sys::signal::kill(self.pid, None).is_ok()
    }

    #[cfg(not(target_os = "linux"))]
    pub fn is_running(&self) -> bool {
        false
    }

    /// Terminate the process (SIGTERM).
    #[cfg(target_os = "linux")]
    pub fn terminate(&self) -> Result<()> {
        use nix::errno::Errno;
        match nix::sys::signal::kill(self.pid, nix::sys::signal::Signal::SIGTERM) {
            Ok(()) => Ok(()),
            Err(Errno::ESRCH) => {
                // Process already exited - that's fine
                tracing::debug!("Process {} already exited, ignoring terminate", self.pid);
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn terminate(&self) -> Result<()> {
        Ok(())
    }

    /// Kill the process (SIGKILL).
    #[cfg(target_os = "linux")]
    pub fn kill(&self) -> Result<()> {
        nix::sys::signal::kill(self.pid, nix::sys::signal::Signal::SIGKILL)?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn kill(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_spawn_config() {
        let config = ContainerProcessConfig {
            container_id: "test".to_string(),
            rootfs: "/".to_string(),
            command: vec!["echo".to_string(), "hello".to_string()],
            workdir: "/".to_string(),
            env: vec![],
            mounts: vec![],
            gpu: false,
        };

        assert_eq!(config.container_id, "test");
        assert_eq!(config.command.len(), 2);
    }
}
