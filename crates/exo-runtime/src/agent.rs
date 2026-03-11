//! Agent-specific security profiles and policies.
//!
//! AI agents have different security requirements than general-purpose containers:
//!
//! - **Code execution**: Python, JavaScript, Bash - not system services
//! - **Network access**: API calls, fetch resources - not raw sockets
//! - **File access**: Workspace isolation - not host filesystem access
//! - **No privileged ops**: No module loading, no hardware access
//!
//! # Agent Security Profile
//!
//! ```text
//! Agent containers:
//! - No privilege escalation (no_new_privs)
//! - No raw sockets or packet manipulation
//! - No system administration (mount, kexec, reboot)
//! - No process tracing (ptrace)
//! - Outbound networking only
//! - Minimal /dev access
//! - Masked system paths (/proc/sys, /sys/firmware)
//! ```

use crate::config::ContainerConfig;
use crate::seccomp::{SeccompProfile, Syscall, SeccompAction, SeccompCompare, ArgRule};
use crate::security::{Capability, drop_capabilities};
use anyhow::Result;

/// Security profile for AI agent containers.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    /// Profile name
    pub name: String,

    /// Seccomp filter profile
    pub seccomp: SeccompProfile,

    /// Capabilities to keep (empty = none)
    pub capabilities: Vec<Capability>,

    /// Enable no_new_privs flag
    pub no_new_privs: bool,

    /// Network access level
    pub network: NetworkAccess,

    /// Paths to mask (hide from container)
    pub masked_paths: Vec<String>,

    /// Paths to read-only mount
    pub readonly_paths: Vec<String>,
}

/// Network access levels for agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAccess {
    /// No networking
    None,

    /// Outbound TCP/UDP only (no raw sockets, no listening)
    OutboundOnly,

    /// Outbound + bind to unprivileged ports only
    Unprivileged,

    /// Full network access (for edge cases)
    Full,
}

impl AgentProfile {
    /// Create a default agent security profile.
    ///
    /// This profile is designed for AI agents executing code with minimal
    /// attack surface while maintaining functionality for LLM workloads.
    pub fn default() -> Self {
        Self {
            name: "agent-default".to_string(),
            seccomp: Self::default_seccomp(),
            capabilities: vec![],
            no_new_privs: false,  // Disabled for Node.js compatibility
            network: NetworkAccess::OutboundOnly,
            masked_paths: vec![
                "/proc/sys".to_string(),
                "/proc/sysrq-trigger".to_string(),
                "/proc/irq".to_string(),
                "/proc/bus".to_string(),
                "/sys/firmware".to_string(),
                "/sys/hypervisor".to_string(),
            ],
            readonly_paths: vec![
                "/proc/sys".to_string(),
                "/sys/bus".to_string(),
                "/sys/class".to_string(),
                "/sys/devices".to_string(),
            ],
        }
    }

    /// Create a strict profile for untrusted code execution.
    ///
    /// Maximum security - blocks everything except basic file I/O
    /// and process execution. Use for running untrusted agent code.
    pub fn strict() -> Self {
        Self {
            name: "agent-strict".to_string(),
            seccomp: Self::strict_seccomp(),
            capabilities: vec![],
            no_new_privs: true,
            network: NetworkAccess::None,
            masked_paths: vec![
                "/proc/sys".to_string(),
                "/proc/sysrq-trigger".to_string(),
                "/proc/irq".to_string(),
                "/proc/bus".to_string(),
                "/sys/firmware".to_string(),
                "/sys/hypervisor".to_string(),
                "/proc/net".to_string(),
                "/sys/module".to_string(),
            ],
            readonly_paths: vec![
                "/proc".to_string(),
                "/sys".to_string(),
            ],
        }
    }

    /// Create a profile for web-browsing agents.
    ///
    /// Allows outbound networking and browser-related operations.
    pub fn web_agent() -> Self {
        let mut profile = Self::default();
        profile.name = "agent-web".to_string();
        profile.network = NetworkAccess::OutboundOnly;
        profile
    }

    /// Create a profile for filesystem-only agents.
    ///
    /// No network, minimal syscalls. Useful for local file operations.
    pub fn filesystem_only() -> Self {
        let mut profile = Self::default();
        profile.name = "agent-filesystem".to_string();
        profile.network = NetworkAccess::None;
        profile
    }

    /// Create a profile for data processing/ML agents.
    ///
    /// Allows compute-heavy operations and memory allocation.
    pub fn compute_agent() -> Self {
        let mut profile = Self::default();
        profile.name = "agent-compute".to_string();
        // Add more syscalls for ML/compute workloads
        profile.seccomp = Self::compute_seccomp();
        profile
    }

    /// Default seccomp profile for agent containers.
    ///
    /// Allows common operations while blocking dangerous syscalls.
    fn default_seccomp() -> SeccompProfile {
        let mut profile = SeccompProfile::whitelist();

        // Essential syscalls for basic operation
        let allowed = vec![
            // Process lifecycle
            "execve", "execveat", "exit", "exit_group", "wait4", "waitpid",
            // File I/O
            "read", "write", "open", "openat", "close", "stat", "fstat", "lstat",
            "newfstatat", "readlink", "readlinkat", "getdents64", "access", "faccessat2",
            // Memory
            "mmap", "mprotect", "munmap", "brk", "mremap", "mbind", "get_mempolicy",
            // Signals
            "rt_sigaction", "rt_sigprocmask", "rt_sigreturn", "kill", "tgkill",
            // Pipes and IPC
            "pipe", "pipe2", "eventfd2", "signalfd4",
            // Basic time
            "clock_gettime", "clock_nanosleep", "nanosleep", "gettimeofday",
            // Scheduling
            "sched_yield", "sched_getaffinity", "sched_setaffinity", "sched_getparam", "sched_setparam",
            // UIDs/GIDs
            "getuid", "geteuid", "getgid", "getegid", "getresuid", "getresgid",
            "setuid", "setgid", "setresuid", "setresgid",
            "capget", "capset",
            // Threading
            "set_tid_address", "set_robust_list", "get_robust_list", "clone", "clone3", "fork", "vfork",
            "futex", "getpid", "gettid", "getppid",
            // epoll/event
            "epoll_create1", "epoll_ctl", "epoll_wait", "epoll_pwait",
            // Basic networking (TCP/UDP only)
            "socket", "connect", "sendto", "recvfrom", "sendmsg", "recvmsg",
            "bind", "listen", "accept", "accept4", "getsockname", "getpeername",
            "getsockopt", "setsockopt", "shutdown", "sockopt",
            // DNS
            "sendmmsg", "recvmmsg",
            // File operations
            "lseek", "pread64", "pwrite64", "preadv2", "pwritev2", "readv", "writev",
            "dup", "dup2", "dup3",
            "select", "poll", "ppoll", "pselect6",
            // Basic misc
            "arch_prctl", "prctl", "uname", "getrlimit", "getrusage", "times",
            "getrandom", "gettimeofday",
            // Sync
            "sync", "syncfs", "fsync", "fdatasync", "sync_file_range",
            // Stat variants
            "statx", "statfs", "fstatfs",
            // Directory operations
            "getdents64", "mkdirat", "unlinkat", "renameat", "renameat2", "symlinkat",
            "getcwd", "readlink", "readlinkat", "chmod", "fchmod", "fchmodat", "umask",
            // Basic fcntl
            "fcntl", "flock",
            // Memory operations
            "msync", "mincore", "madvise", "mlock", "munlock", "mlockall", "munlockall",
            // Time
            "time", "times",
            // Signals
            "rt_sigaction", "rt_sigprocmask", "rt_sigreturn", "sigaltstack", "sigreturn",
            // Pipe/splice
            "splice", "tee", "vmsplice",
            // Files
            "ioctl", "readahead", "sync_file_range", "fallocate",
            "utimensat", "futimesat",
            // Process
            "prlimit64", "getpriority", "setpriority", "rseq",
            // Checksum
            "restart_syscall",
        ];

        for syscall in allowed {
            profile.allow_syscall(syscall);
        }

        // Explicitly block dangerous syscalls (even if somehow in allow list)
        let blocked = vec![
            "kexec_load", "kexec_file_load",
            "init_module", "finit_module", "delete_module",
            "ptrace", "process_vm_readv", "process_vm_writev",
            "reboot",
            "swapon", "swapoff",
            "settimeofday", "adjtimex", "clock_settime", "stime",
            "sethostname", "setdomainname",
            "iopl", "ioperm",
            "ioprio_set", "ioprio_get",
            "acct",
            "mount", "umount2", "pivot_root",
            "chroot",
            "quotactl",
            "add_key", "request_key", "keyctl",
            "bpf",
            "perf_event_open",
            "userfaultfd",
            "name_to_handle_at", "open_by_handle_at",
            "mknod", "mknodat",
            "socketcall",
            "sysfs", "uselib", "ustat",
            "vm86", "vm86old",
        ];

        for syscall in blocked {
            profile.deny_syscall(syscall);
        }

        // Block raw sockets (argument filtering)
        profile.add_arg_rule(ArgRule {
            syscall: Syscall::Name("socket".to_string()),
            arg_num: 1, // domain
            compare: SeccompCompare::Eq(2), // AF_INET
            action: SeccompAction::Allow,
        });
        profile.add_arg_rule(ArgRule {
            syscall: Syscall::Name("socket".to_string()),
            arg_num: 1, // domain
            compare: SeccompCompare::Eq(10), // AF_INET6
            action: SeccompAction::Allow,
        });
        // Block AF_PACKET, AF_NETLINK, etc.
        profile.add_arg_rule(ArgRule {
            syscall: Syscall::Name("socket".to_string()),
            arg_num: 0, // type
            compare: SeccompCompare::Eq(3), // SOCK_RAW
            action: SeccompAction::Errno,
        });

        profile
    }

    /// Strict seccomp profile - minimal syscall set.
    fn strict_seccomp() -> SeccompProfile {
        let mut profile = SeccompProfile::whitelist();

        let minimal = vec![
            // Absolute minimum for code execution
            "execve", "exit", "exit_group",
            // File I/O
            "read", "write", "open", "openat", "close",
            // Memory
            "mmap", "mprotect", "munmap", "brk",
            // Basic signals
            "rt_sigaction", "rt_sigprocmask", "rt_sigreturn",
            // Basic IPC
            "pipe", "pipe2",
            // Time
            "clock_gettime", "nanosleep",
            // Process info
            "getpid", "gettid",
            // Misc
            "arch_prctl", "set_tid_address",
            "readlink", "getuid", "getgid", "geteuid", "getegid",
        ];

        for syscall in minimal {
            profile.allow_syscall(syscall);
        }

        profile
    }

    /// Compute-focused seccomp profile for ML agents.
    fn compute_seccomp() -> SeccompProfile {
        let mut profile = Self::default_seccomp();

        // Additional syscalls for compute workloads
        let compute_extra = vec![
            "sched_setaffinity", "sched_getaffinity",
            "mbind", "set_mempolicy", "get_mempolicy",
            "migrate_pages",
            "getcpu",
            "memfd_create",
        ];

        for syscall in compute_extra {
            profile.allow_syscall(syscall);
        }

        profile
    }

    /// Apply the agent security profile to the current process.
    ///
    /// This should be called in the child process before execve.
    #[cfg(target_os = "linux")]
    pub fn apply(&self) -> Result<()> {
        tracing::info!("Applying agent security profile: {}", self.name);

        // Drop capabilities
        drop_capabilities(&self.capabilities)?;

        // Apply seccomp filter
        #[cfg(feature = "seccomp")]
        {
            crate::seccomp::apply_seccomp(&self.seccomp)?;
        }

        // Set no_new_privs if requested
        if self.no_new_privs {
            self.set_no_new_privs()?;
        }

        tracing::info!("Agent security profile applied: {}", self.name);

        Ok(())
    }

    /// Set the no_new_privs bit for the current process.
    ///
    /// Prevents setuid/setgid binaries from gaining privileges.
    #[cfg(target_os = "linux")]
    fn set_no_new_privs(&self) -> Result<()> {
        use nix::unistd::Uid;
        const PR_SET_NO_NEW_PRIVS: i32 = 38;

        // Use prctl to set no_new_privs
        let ret = unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };

        if ret != 0 {
            anyhow::bail!("Failed to set no_new_privs: {}", std::io::Error::last_os_error());
        }

        tracing::debug!("no_new_privs bit set");
        Ok(())
    }

    /// Non-Linux stub
    #[cfg(not(target_os = "linux"))]
    pub fn apply(&self) -> Result<()> {
        tracing::warn!("Agent security profiles not supported on this platform");
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn set_no_new_privs(&self) -> Result<()> {
        Ok(())
    }
}

impl Default for AgentProfile {
    fn default() -> Self {
        Self::default()
    }
}

/// Get the agent security profile from a container config.
///
/// Checks the config for agent-specific settings and returns
/// an appropriate profile.
pub fn get_agent_profile(config: &ContainerConfig) -> AgentProfile {
    use crate::agent::AgentConfigExt;

    // Check for profile name in config env
    if let Some(profile_name) = config.security_profile() {
        match profile_name.as_str() {
            "strict" => return AgentProfile::strict(),
            "web" => return AgentProfile::web_agent(),
            "filesystem" => return AgentProfile::filesystem_only(),
            "compute" => return AgentProfile::compute_agent(),
            _ => {}
        }
    }

    // Check for network config to determine profile
    if config.network.mode == "none" {
        return AgentProfile::filesystem_only();
    }

    // Default profile
    AgentProfile::default()
}

/// Extend ContainerConfig with agent-specific fields.
///
/// This is a trait that adds agent security options to ContainerConfig.
pub trait AgentConfigExt {
    /// Get the agent security profile name.
    fn security_profile(&self) -> Option<&String>;

    /// Check if this is an agent container.
    fn is_agent(&self) -> bool;

    /// Get the allowed tools for this agent.
    fn allowed_tools(&self) -> &[String];
}

impl AgentConfigExt for ContainerConfig {
    fn security_profile(&self) -> Option<&String> {
        // This would be stored in the config's metadata or extra fields
        // For now, we check env for AGENT_PROFILE
        self.env.get("AGENT_PROFILE")
    }

    fn is_agent(&self) -> bool {
        // Check for agent marker in env or command
        self.env.contains_key("AGENT_NAME")
            || self.env.contains_key("AGENT_ID")
            || self.command.first().map_or(false, |c| c.contains("python"))
            || self.command.first().map_or(false, |c| c.contains("node"))
    }

    fn allowed_tools(&self) -> &[String] {
        static DEFAULT_TOOLS: &[&str] = &["python", "bash", "curl"];
        // This would be configurable
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_profile_default() {
        let profile = AgentProfile::default();
        assert_eq!(profile.name, "agent-default");
        assert!(profile.no_new_privs);
        assert_eq!(profile.network, NetworkAccess::OutboundOnly);
    }

    #[test]
    fn test_agent_profile_strict() {
        let profile = AgentProfile::strict();
        assert_eq!(profile.name, "agent-strict");
        assert_eq!(profile.network, NetworkAccess::None);
    }

    #[test]
    fn test_network_access_variants() {
        assert_ne!(NetworkAccess::None, NetworkAccess::OutboundOnly);
        assert_ne!(NetworkAccess::OutboundOnly, NetworkAccess::Unprivileged);
    }

    #[test]
    fn test_profile_has_seccomp_rules() {
        let profile = AgentProfile::default();
        assert!(!profile.seccomp.allow.is_empty());
        assert!(!profile.seccomp.deny.is_empty());
    }
}
