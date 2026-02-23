//! Linux namespace operations for container isolation.

#[cfg(target_os = "linux")]
use nix::sched::{CloneFlags, unshare, setns};
#[cfg(target_os = "linux")]
use nix::unistd::Pid;
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
use anyhow::Result;

/// Linux namespace types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    Mount,   // CLONE_NEWNS - Mount namespace
    Uts,     // CLONE_NEWUTS - UTS namespace (hostname)
    Ipc,     // CLONE_NEWIPC - IPC namespace
    Network, // CLONE_NEWNET - Network namespace
    Pid,     // CLONE_NEWPID - PID namespace
    User,    // CLONE_NEWUSER - User namespace
    Cgroup,  // CLONE_NEWCGROUP - Cgroup namespace
}

#[cfg(target_os = "linux")]
impl Namespace {
    /// Get the clone flag for this namespace.
    pub fn clone_flag(&self) -> CloneFlags {
        match self {
            Namespace::Mount => CloneFlags::CLONE_NEWNS,
            Namespace::Uts => CloneFlags::CLONE_NEWUTS,
            Namespace::Ipc => CloneFlags::CLONE_NEWIPC,
            Namespace::Network => CloneFlags::CLONE_NEWNET,
            Namespace::Pid => CloneFlags::CLONE_NEWPID,
            Namespace::User => CloneFlags::CLONE_NEWUSER,
            Namespace::Cgroup => CloneFlags::CLONE_NEWCGROUP,
        }
    }

    /// Get the namespace file path for a process.
    pub fn path_for(&self, pid: Pid) -> String {
        let ns_type = match self {
            Namespace::Mount => "mnt",
            Namespace::Uts => "uts",
            Namespace::Ipc => "ipc",
            Namespace::Network => "net",
            Namespace::Pid => "pid",
            Namespace::User => "user",
            Namespace::Cgroup => "cgroup",
        };
        format!("/proc/{}/ns/{}", pid.as_raw(), ns_type)
    }
}

#[cfg(not(target_os = "linux"))]
impl Namespace {
    pub fn clone_flag(&self) -> u32 {
        0
    }

    pub fn path_for(&self, _pid: u32) -> String {
        format!("/proc/self/ns/{}", self.as_str())
    }

    fn as_str(&self) -> &'static str {
        match self {
            Namespace::Mount => "mnt",
            Namespace::Uts => "uts",
            Namespace::Ipc => "ipc",
            Namespace::Network => "net",
            Namespace::Pid => "pid",
            Namespace::User => "user",
            Namespace::Cgroup => "cgroup",
        }
    }
}

/// Unshare the given namespaces for the current process.
#[cfg(target_os = "linux")]
pub fn unshare_namespaces(namespaces: &[Namespace]) -> Result<()> {
    let mut flags = CloneFlags::empty();

    for ns in namespaces {
        // Don't unshare PID namespace here - requires fork/clone
        if *ns != Namespace::Pid {
            flags |= ns.clone_flag();
        }
    }

    if !flags.is_empty() {
        unshare(flags)?;
        tracing::debug!("Unshared namespaces: {:?}", flags);
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn unshare_namespaces(_namespaces: &[Namespace]) -> Result<()> {
    Err(anyhow::anyhow!("Namespaces only supported on Linux"))
}

/// Enter an existing namespace (for exec/attach).
#[cfg(target_os = "linux")]
pub fn enter_namespace(pid: Pid, ns_type: Namespace) -> Result<()> {
    let path = ns_type.path_for(pid);
    let file = File::open(&path)?;
    setns(file.as_raw_fd(), ns_type.clone_flag())?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn enter_namespace(_pid: u32, _ns_type: Namespace) -> Result<()> {
    Err(anyhow::anyhow!("Namespaces only supported on Linux"))
}

/// Get combined clone flags for multiple namespaces.
#[cfg(target_os = "linux")]
pub fn clone_flags_for(namespaces: &[Namespace]) -> CloneFlags {
    let mut flags = CloneFlags::empty();
    for ns in namespaces {
        flags |= ns.clone_flag();
    }
    flags
}

#[cfg(not(target_os = "linux"))]
pub fn clone_flags_for(_namespaces: &[Namespace]) -> u32 {
    0
}

/// Set hostname in UTS namespace.
#[cfg(target_os = "linux")]
pub fn set_hostname(hostname: &str) -> Result<()> {
    nix::unistd::sethostname(hostname)?;
    tracing::debug!("Set hostname: {}", hostname);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn set_hostname(_hostname: &str) -> Result<()> {
    // Stub for non-Linux
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_flags() {
        #[cfg(target_os = "linux")]
        {
            assert_eq!(Namespace::Mount.clone_flag(), nix::sched::CloneFlags::CLONE_NEWNS);
            assert_eq!(Namespace::Uts.clone_flag(), nix::sched::CloneFlags::CLONE_NEWUTS);
        }
    }
}
