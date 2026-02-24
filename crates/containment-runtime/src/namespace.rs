//! Linux namespace operations.

#[cfg(target_os = "linux")]
use nix::sched::{CloneFlags, setns, unshare};
#[cfg(target_os = "linux")]
use nix::unistd::Pid;
use std::fs::File;
use anyhow::Result;

/// Process ID type (platform-specific).
#[cfg(target_os = "linux")]
pub use nix::unistd::Pid as ProcessId;

/// Process ID type (non-Linux stub).
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy)]
pub struct ProcessId(i32);

/// Represents a Linux namespace type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    /// Mount namespace - isolate filesystem mount points
    Mount,
    /// UTS namespace - isolate hostname and domain name
    Uts,
    /// IPC namespace - isolate Inter-Process Communication
    Ipc,
    /// Network namespace - isolate network interfaces
    Network,
    /// PID namespace - isolate process IDs
    Pid,
    /// User namespace - isolate user and group IDs
    User,
    /// Cgroup namespace - isolate cgroup root directory
    Cgroup,
}

/// Clone flags for namespace creation.
#[cfg(target_os = "linux")]
pub use nix::sched::CloneFlags as NamespaceFlags;

/// Stub clone flags for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy)]
pub struct NamespaceFlags;

#[cfg(not(target_os = "linux"))]
impl NamespaceFlags {
    pub const fn empty() -> Self { Self }
}

#[cfg(target_os = "linux")]
impl Namespace {
    /// Get the corresponding CloneFlag for this namespace type.
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

    /// Get the namespace file path for a given process.
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

    /// Get the standard path for this namespace type (for self).
    pub fn path(&self) -> String {
        self.path_for(Pid::from_raw(1))
    }
}

#[cfg(not(target_os = "linux"))]
impl Namespace {
    /// Stub implementation for non-Linux platforms.
    pub fn clone_flag(&self) -> NamespaceFlags {
        NamespaceFlags
    }

    /// Stub implementation for non-Linux platforms.
    pub fn path_for(&self, pid: ProcessId) -> String {
        format!("/proc/{}/ns/{}", pid.0, self.as_str())
    }

    /// Stub implementation for non-Linux platforms.
    pub fn path(&self) -> String {
        self.path_for(ProcessId(1))
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

/// Create new namespaces by calling unshare(2).
#[cfg(target_os = "linux")]
pub fn unshare_namespaces(namespaces: &[Namespace]) -> Result<()> {
    let mut flags = CloneFlags::empty();

    for ns in namespaces {
        flags |= ns.clone_flag();
    }

    // Don't unshare PID namespace here - it requires fork()
    let pid_ns = namespaces.iter().any(|n| *n == Namespace::Pid);
    let mut filtered_flags = flags;
    if pid_ns {
        filtered_flags.remove(CloneFlags::CLONE_NEWPID);
    }

    if !filtered_flags.is_empty() {
        unshare(filtered_flags)?;
    }

    Ok(())
}

/// Stub implementation for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn unshare_namespaces(_namespaces: &[Namespace]) -> Result<()> {
    Err(anyhow::anyhow!("Namespaces are only supported on Linux"))
}

/// Enter an existing namespace using setns(2).
#[cfg(target_os = "linux")]
pub fn enter_namespace(pid: Pid, ns_type: Namespace) -> Result<()> {
    let path = ns_type.path_for(pid);
    let file = File::open(&path)?;
    setns(&file, ns_type.clone_flag())?;
    Ok(())
}

/// Stub implementation for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn enter_namespace(_pid: ProcessId, _ns_type: Namespace) -> Result<()> {
    Err(anyhow::anyhow!("Namespaces are only supported on Linux"))
}

/// Create new namespaces for a child process via clone(2).
#[cfg(target_os = "linux")]
pub fn clone_with_namespaces(namespaces: &[Namespace]) -> CloneFlags {
    let mut flags = CloneFlags::empty();

    for ns in namespaces {
        flags |= ns.clone_flag();
    }

    flags
}

/// Stub implementation for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn clone_with_namespaces(_namespaces: &[Namespace]) -> NamespaceFlags {
    NamespaceFlags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_clone_flags() {
        #[cfg(target_os = "linux")]
        {
            assert_eq!(Namespace::Mount.clone_flag(), CloneFlags::CLONE_NEWNS);
            assert_eq!(Namespace::Uts.clone_flag(), CloneFlags::CLONE_NEWUTS);
            assert_eq!(Namespace::Ipc.clone_flag(), CloneFlags::CLONE_NEWIPC);
            assert_eq!(Namespace::Network.clone_flag(), CloneFlags::CLONE_NEWNET);
            assert_eq!(Namespace::Pid.clone_flag(), CloneFlags::CLONE_NEWPID);
            assert_eq!(Namespace::User.clone_flag(), CloneFlags::CLONE_NEWUSER);
        }
    }

    #[test]
    fn test_namespace_path() {
        #[cfg(target_os = "linux")]
        {
            assert_eq!(Namespace::Mount.path_for(Pid::from_raw(1234)), "/proc/1234/ns/mnt");
            assert_eq!(Namespace::Network.path_for(Pid::from_raw(1)), "/proc/1/ns/net");
        }
    }
}
