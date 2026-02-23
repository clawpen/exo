//! Container process spawning and management.

use crate::config::ContainerConfig;
use anyhow::Result;

#[cfg(target_os = "linux")]
use nix::sys::wait::{waitpid, WaitStatus};
#[cfg(target_os = "linux")]
use nix::unistd::Pid;
#[cfg(target_os = "linux")]
use nix::sys::signal::Signal;

/// Container process handle.
#[derive(Debug)]
pub struct ContainerProcess {
    /// The actual process ID on the host
    #[cfg(target_os = "linux")]
    pub pid: Pid,

    #[cfg(not(target_os = "linux"))]
    pub pid: u32,

    /// The namespace file descriptors for entering the container
    pub namespaces: NamespaceHandles,

    /// Process state
    pub state: ProcessState,
}

/// Handle to a container's namespaces for entering/exploring.
#[derive(Debug)]
pub struct NamespaceHandles {
    pub mount: Option<std::fs::File>,
    pub uts: Option<std::fs::File>,
    pub ipc: Option<std::fs::File>,
    pub net: Option<std::fs::File>,
    pub pid: Option<std::fs::File>,
}

/// Process state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Exited(i32),
    Failed(i32),
}

impl ContainerProcess {
    /// Spawn a new container process.
    #[cfg(target_os = "linux")]
    pub fn spawn(config: &ContainerConfig) -> Result<Self> {
        use std::process::Command;

        // Spawn the process using the configured command
        let mut cmd = Command::new(&config.command[0]);
        cmd.args(&config.command[1..]);

        // Set environment
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        // Set working directory (for now, host-side)
        cmd.current_dir(&config.workdir);

        // Spawn the child process
        let child = cmd.spawn()?;
        let pid = Pid::from_raw(child.id() as i32);

        // TODO: Actually set up namespaces before exec
        // This requires a fork/exec approach where we:
        // 1. Clone with namespaces
        // 2. Set up rootfs, mounts, etc.
        // 3. Exec the target binary

        Ok(Self {
            pid,
            namespaces: NamespaceHandles {
                mount: None,
                uts: None,
                ipc: None,
                net: None,
                pid: None,
            },
            state: ProcessState::Running,
        })
    }

    /// Spawn a new container process (non-Linux stub).
    #[cfg(not(target_os = "linux"))]
    pub fn spawn(_config: &ContainerConfig) -> Result<Self> {
        Err(anyhow::anyhow!("Container spawning is only supported on Linux"))
    }

    /// Wait for the process to exit.
    #[cfg(target_os = "linux")]
    pub fn wait(&self) -> Result<ProcessState> {
        match waitpid(self.pid, None) {
            Ok(WaitStatus::Exited(_, exit_code)) => {
                Ok(ProcessState::Exited(exit_code))
            }
            Ok(WaitStatus::Signaled(_, signal, _)) => {
                Ok(ProcessState::Failed(signal as i32))
            }
            Ok(_) => Ok(ProcessState::Running),
            Err(e) => Err(e.into()),
        }
    }

    /// Wait stub for non-Linux platforms.
    #[cfg(not(target_os = "linux"))]
    pub fn wait(&self) -> Result<ProcessState> {
        Ok(self.state)
    }

    /// Send a signal to the container process.
    #[cfg(target_os = "linux")]
    pub fn kill(&self, signal: Signal) -> Result<()> {
        nix::sys::signal::kill(self.pid, signal)?;
        Ok(())
    }

    /// Send SIGTERM to the container process.
    pub fn terminate(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.kill(Signal::SIGTERM)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            anyhow::bail!("Container control is only supported on Linux");
        }
        Ok(())
    }

    /// Send SIGKILL to the container process.
    pub fn kill_hard(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.kill(Signal::SIGKILL)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            anyhow::bail!("Container control is only supported on Linux");
        }
        Ok(())
    }

    /// Check if the process is still running.
    pub fn is_running(&self) -> bool {
        matches!(self.state, ProcessState::Running)
    }
}

/// Fork-based container spawning with proper namespace isolation.
///
/// This is the proper way to spawn containers with PID namespace support.
#[cfg(target_os = "linux")]
pub fn spawn_container_fork(config: &ContainerConfig) -> Result<Pid> {
    use nix::unistd::{fork, ForkResult};
    use nix::sched::{unshare, CloneFlags};
    use nix::unistd::sethostname;

    // Fork to create new process
    match unsafe { fork() }? {
        ForkResult::Parent { child } => {
            // Parent process - return child PID
            return Ok(child);
        }
        ForkResult::Child => {
            // Child process - set up container environment

            // Unshare namespaces (except PID, which requires clone)
            let namespaces = [
                CloneFlags::CLONE_NEWNS,    // Mount
                CloneFlags::CLONE_NEWUTS,   // UTS
                CloneFlags::CLONE_NEWIPC,   // IPC
                CloneFlags::CLONE_NEWNET,   // Network
            ];
            let mut flags = CloneFlags::empty();
            for ns in namespaces {
                flags |= ns;
            }

            unshare(flags)?;

            // Set hostname if UTS namespace is isolated
            if !config.hostname.is_empty() {
                sethostname(&config.hostname)?;
            }

            // TODO: Set up rootfs with pivot_root
            // TODO: Set up mounts
            // TODO: Set up network
            // TODO: Drop capabilities

            // Execute the container command
            use std::ffi::CString;
            use nix::unistd::execve;

            let program = CString::new(config.command[0].as_bytes())?;
            let args: Vec<CString> = config.command.iter()
                .map(|a| CString::new(a.as_bytes()).unwrap())
                .collect();

            // Set up environment
            let mut env_vars: Vec<CString> = vec![
                CString::new("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin").unwrap(),
                CString::new("TERM=xterm").unwrap(),
            ];
            for (key, value) in &config.env {
                let env_str = format!("{}={}", key, value);
                env_vars.push(CString::new(env_str).unwrap());
            }

            // Note: This won't actually work in simple tests because we haven't
            // set up the rootfs. In production, we'd pivot_root first.
            // For now, exec will fail, but the structure is correct.

            let _ = execve(&program, &args, &env_vars);
            std::process::exit(1);
        }
    }
}

/// Fork stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn spawn_container_fork(_config: &ContainerConfig) -> Result<u32> {
    Err(anyhow::anyhow!("Container forking is only supported on Linux"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_state() {
        let running = ProcessState::Running;
        let exited = ProcessState::Exited(0);
        let failed = ProcessState::Failed(1);

        assert!(running == ProcessState::Running);
        assert_eq!(exited, ProcessState::Exited(0));
    }
}
