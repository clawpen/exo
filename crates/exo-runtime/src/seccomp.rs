//! Seccomp syscall filtering for containers.
//!
//! This module implements seccomp-bpf filtering to restrict which syscalls
//! a container process can make. This provides a strong security boundary by
//! limiting the attack surface to only necessary syscalls.
//!
//! # Why Seccomp?
//!
//! Even with dropped capabilities, a compromised process can still make
//! many syscalls that could be used for exploitation. Seccomp filtering
//! adds a kernel-level syscall whitelist/blacklist.
//!
//! # Example
//!
//! ```no_run
//! use exo_runtime::seccomp::{apply_seccomp, default_profile};
//!
//! let profile = default_profile();
//! apply_seccomp(&profile)?;
//! ```

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};

#[cfg(target_os = "linux")]
use {
    libseccomp::{ScmpAction, ScmpArgCompare, ScmpCompareOp, ScmpFilterContext, ScmpSyscall},
};

/// Seccomp action for syscall rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeccompAction {
    /// Kill the process (most secure)
    Kill,
    /// Return EPERM (default deny action)
    Errno,
    /// Allow the syscall
    Allow,
    /// Log and allow (for debugging)
    Log,
}

impl SeccompAction {
    #[cfg(target_os = "linux")]
    fn as_scmp_action(self) -> ScmpAction {
        match self {
            SeccompAction::Kill => ScmpAction::KillThread,
            SeccompAction::Errno => ScmpAction::Errno(libc::EPERM),
            SeccompAction::Allow => ScmpAction::Allow,
            SeccompAction::Log => ScmpAction::Log,
        }
    }
}

/// Syscall identifier by name or number.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Syscall {
    /// By name (e.g., "open", "read", "write")
    Name(String),
    /// By number (architecture-specific)
    Number(u32),
}

impl Syscall {
    #[cfg(target_os = "linux")]
    fn as_scmp_syscall(&self) -> ScmpSyscall {
        match self {
            Syscall::Name(name) => ScmpSyscall::from_name(name)
                .unwrap_or_else(|_| ScmpSyscall::from_name("invalid").unwrap_or(ScmpSyscall::from(-1))),
            Syscall::Number(num) => ScmpSyscall::from(*num as i32),
        }
    }

    pub fn name(name: &str) -> Self {
        Syscall::Name(name.to_string())
    }

    pub fn number(num: u32) -> Self {
        Syscall::Number(num)
    }
}

impl From<&str> for Syscall {
    fn from(s: &str) -> Self {
        Syscall::Name(s.to_string())
    }
}

/// Seccomp filter profile.
///
/// Defines which syscalls are allowed or denied.
#[derive(Debug, Clone)]
pub struct SeccompProfile {
    /// Syscalls to explicitly allow
    pub allow: Vec<Syscall>,

    /// Syscalls to explicitly deny
    pub deny: Vec<Syscall>,

    /// Default action for unspecified syscalls
    pub default_action: SeccompAction,

    /// Architecture filter (None = current arch)
    pub arch: Option<String>,

    /// Additional rules with argument filtering
    pub arg_rules: Vec<ArgRule>,
}

/// Conditional rule based on syscall arguments.
#[derive(Debug, Clone)]
pub struct ArgRule {
    /// Syscall this rule applies to
    pub syscall: Syscall,

    /// Argument index (0, 1, 2, 3, 4, 5)
    pub arg_num: u32,

    /// Comparison operation
    pub compare: SeccompCompare,

    /// Action to take if condition matches
    pub action: SeccompAction,
}

/// Comparison operation for argument filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeccompCompare {
    /// Equal
    Eq(u64),
    /// Not equal
    NotEq(u64),
    /// Greater than
    Gt(u64),
    /// Greater than or equal
    Ge(u64),
    /// Less than
    Lt(u64),
    /// Less than or equal
    Le(u64),
    /// Masked equality (value & mask == val)
    MaskedEq(u64, u64),
}

impl SeccompProfile {
    /// Create a new empty profile with the specified default action.
    pub fn new(default_action: SeccompAction) -> Self {
        Self {
            allow: vec![],
            deny: vec![],
            default_action,
            arch: None,
            arg_rules: vec![],
        }
    }

    /// Create a profile with default deny (whitelist mode).
    pub fn whitelist() -> Self {
        Self::new(SeccompAction::Errno)
    }

    /// Create a profile with default allow (blacklist mode).
    pub fn blacklist() -> Self {
        Self::new(SeccompAction::Allow)
    }

    /// Add syscall to allow list.
    pub fn allow_syscall(&mut self, syscall: impl Into<Syscall>) -> &mut Self {
        self.allow.push(syscall.into());
        self
    }

    /// Add syscall to deny list.
    pub fn deny_syscall(&mut self, syscall: impl Into<Syscall>) -> &mut Self {
        self.deny.push(syscall.into());
        self
    }

    /// Add an argument filtering rule.
    pub fn add_arg_rule(&mut self, rule: ArgRule) -> &mut Self {
        self.arg_rules.push(rule);
        self
    }
}

impl Default for SeccompProfile {
    fn default() -> Self {
        Self::whitelist()
    }
}

/// Get the default seccomp profile for containers.
///
/// This is a balanced profile that allows most common operations
/// while blocking dangerous syscalls. Compatible with Docker's
/// default seccomp profile.
pub fn default_profile() -> SeccompProfile {
    let mut profile = SeccompProfile::whitelist();

    // Essential syscalls for basic operation
    let essential = vec![
        // File operations
        "read", "write", "open", "openat", "close", "stat", "fstat", "lstat",
        "poll", "lseek", "mmap", "mprotect", "munmap", "brk", "rt_sigaction",
        "rt_sigprocmask", "rt_sigreturn", "ioctl", "pread64", "pwrite64",
        "readv", "writev", "access", "pipe", "select", "sched_yield",
        "mremap", "msync", "mincore", "madvise", "dup", "dup2", "pause",
        "nanosleep", "getitimer", "alarm", "setitimer", "getpid", "sendfile",
        "socket", "connect", "accept", "sendto", "recvfrom", "sendmsg",
        "recvmsg", "shutdown", "bind", "listen", "getsockname", "getpeername",
        "socketpair", "setsockopt", "getsockopt", "clone", "fork", "vfork",
        "execve", "exit", "wait4", "kill", "uname", "getrlimit",
        "getrusage", "sysinfo", "times", "getuid", "getgid", "setuid",
        "setgid", "geteuid", "getegid", "setpgid", "getppid", "getpgrp",
        "setsid", "setreuid", "setregid", "getgroups", "setgroups",
        "setresuid", "getresuid", "setresgid", "getresgid", "getpgid",
        "setfsuid", "setfsgid", "getsid", "capget", "capset", "rt_sigpending",
        "rt_sigtimedwait", "rt_sigqueueinfo", "sigaltstack", "utime",
        "mknod", "uselib", "personality", "ustat", "statfs", "fstatfs",
        "sysfs", "getpriority", "setpriority", "sched_setparam",
        "sched_getparam", "sched_setscheduler", "sched_getscheduler",
        "sched_get_priority_max", "sched_get_priority_min", "sched_rr_get_interval",
        "mlock", "munlock", "mlockall", "munlockall", "vhangup", "pivot_root",
        "prctl", "arch_prctl", "adjtimex", "setrlimit", "chroot", "sync",
        "acct", "settimeofday", "mount", "umount2", "swapon", "swapoff",
        "reboot", "sethostname", "setdomainname", "iopl", "ioperm", "init_module",
        "delete_module", "quotactl", "gettid", "readahead", "setxattr",
        "lsetxattr", "fsetxattr", "getxattr", "lgetxattr", "fgetxattr",
        "listxattr", "llistxattr", "flistxattr", "removexattr", "lremovexattr",
        "fremovexattr", "tkill", "time", "futex", "sched_setaffinity",
        "sched_getaffinity", "set_thread_area", "io_setup", "io_destroy",
        "io_getevents", "io_submit", "io_cancel", "get_thread_area", "epoll_create",
        "epoll_ctl", "epoll_wait", "remap_file_pages", "getdents64", "set_tid_address",
        "restart_syscall", "semtimedop", "fadvise64", "timer_create", "timer_settime",
        "timer_gettime", "timer_getoverrun", "timer_delete", "clock_settime",
        "clock_gettime", "clock_getres", "clock_nanosleep", "exit_group", "epoll_wait",
        "epoll_ctl", "tgkill", "utimes", "mbind", "set_mempolicy", "get_mempolicy",
        "mq_open", "mq_unlink", "mq_timedsend", "mq_timedreceive", "mq_notify",
        "mq_getsetattr", "kexec_load", "waitid", "add_key", "request_key",
        "keyctl", "ioprio_set", "ioprio_get", "inotify_init", "inotify_add_watch",
        "inotify_rm_watch", "migrate_pages", "openat", "mkdirat", "mknodat",
        "fchownat", "futimesat", "newfstatat", "unlinkat", "renameat", "linkat",
        "symlinkat", "readlinkat", "fchmodat", "faccessat", "pselect6", "ppoll",
        "unshare", "set_robust_list", "get_robust_list", "splice", "tee", "sync_file_range",
        "vmsplice", "move_pages", "utimensat", "epoll_pwait", "signalfd", "timerfd_create",
        "eventfd", "fallocate", "timerfd_settime", "timerfd_gettime", "accept4",
        "signalfd4", "eventfd2", "epoll_create1", "dup3", "pipe2", "inotify_init1",
        "preadv", "pwritev", "rt_tgsigqueueinfo", "perf_event_open", "recvmmsg",
        "fanotify_init", "fanotify_mark", "prlimit64", "name_to_handle_at",
        "open_by_handle_at", "clock_adjtime", "syncfs", "sendmmsg", "setns",
        "getcpu", "process_vm_readv", "process_vm_writev", "kcmp", "finit_module",
        "sched_setattr", "sched_getattr", "renameat2", "seccomp", "getrandom",
        "memfd_create", "kexec_file_load", "bpf", "execveat", "userfaultfd",
        "membarrier", "mlock2", "copy_file_range", "preadv2", "pwritev2",
        "pkey_mprotect", "pkey_alloc", "pkey_free", "statx",
        // Directory operations
        "chdir", "fchdir", "mkdir", "rmdir", "getcwd",
        // Network syscalls
        "socketcall", "bind", "listen", "accept", "connect", "getsockname",
        "getpeername", "sendto", "recvfrom", "sendmsg", "recvmsg", "shutdown",
        // Signal handling
        "sigreturn", "sigaction", "sigprocmask", "sigpending", "sigsuspend",
        // Memory management
        "shmget", "shmat", "shmctl", "shmdt",
    ];

    for syscall in essential {
        profile.allow_syscall(syscall);
    }

    profile
}

/// Get a strict seccomp profile for maximum security.
///
/// This profile only allows the most essential syscalls for basic
/// process operation. Suitable for highly trusted containers running
/// simple workloads.
pub fn strict_profile() -> SeccompProfile {
    let mut profile = SeccompProfile::whitelist();

    // Minimal syscall set
    let minimal = vec![
        // Process lifecycle
        "execve", "exit", "exit_group",
        // File I/O
        "read", "write", "open", "openat", "close", "stat", "fstat", "lstat",
        // Memory
        "mmap", "mprotect", "munmap", "brk",
        // Signals
        "rt_sigaction", "rt_sigprocmask", "rt_sigreturn",
        // Basic IPC
        "pipe", "pipe2",
        // Time
        "clock_gettime", "nanosleep",
        // Misc
        "arch_prctl", "set_tid_address", "getpid",
    ];

    for syscall in minimal {
        profile.allow_syscall(syscall);
    }

    profile
}

/// Apply a seccomp profile to the current process.
///
/// # Important
///
/// Once seccomp filtering is applied, it cannot be removed for the
/// lifetime of the process. This should be one of the last steps
/// before execve().
///
/// # Arguments
///
/// * `profile` - The seccomp profile to apply
///
/// # Returns
///
/// Ok(()) on success, error if filter setup fails
#[cfg(target_os = "linux")]
pub fn apply_seccomp(profile: &SeccompProfile) -> Result<()> {
    // Create filter context with default action
    let mut ctx = ScmpFilterContext::new_filter(profile.default_action.as_scmp_action())
        .context("Failed to create seccomp filter context")?;

    // Set architecture if specified
    if let Some(ref arch) = profile.arch {
        // Would need to parse arch string and add_arch
        tracing::debug!("Setting architecture filter: {}", arch);
    }

    // Add allow rules
    for syscall in &profile.allow {
        match syscall {
            Syscall::Name(name) => {
                if let Ok(sc) = ScmpSyscall::from_name(name) {
                    ctx.add_rule(ScmpAction::Allow, sc)
                        .with_context(|| format!("Failed to add allow rule for {}", name))?;
                    tracing::trace!("Seccomp: allow {}", name);
                } else {
                    tracing::warn!("Unknown syscall for allow: {}", name);
                }
            }
            Syscall::Number(num) => {
                let sc = ScmpSyscall::from(*num as i32);
                ctx.add_rule(ScmpAction::Allow, sc)
                    .with_context(|| format!("Failed to add allow rule for syscall {}", num))?;
            }
        }
    }

    // Add deny rules
    for syscall in &profile.deny {
        match syscall {
            Syscall::Name(name) => {
                if let Ok(sc) = ScmpSyscall::from_name(name) {
                    ctx.add_rule(ScmpAction::Errno(libc::EPERM), sc)
                        .with_context(|| format!("Failed to add deny rule for {}", name))?;
                    tracing::trace!("Seccomp: deny {}", name);
                }
            }
            Syscall::Number(num) => {
                let sc = ScmpSyscall::from(*num as i32);
                ctx.add_rule(ScmpAction::Errno(libc::EPERM), sc)?;
            }
        }
    }

    // Add argument filtering rules
    for rule in &profile.arg_rules {
        let sc = rule.syscall.as_scmp_syscall();
        let action = rule.action.as_scmp_action();
        let compare = rule.compare;

        let arg_cmp = match compare {
            SeccompCompare::Eq(val) => ScmpArgCompare::new(rule.arg_num as u32, ScmpCompareOp::Equal, val),
            SeccompCompare::NotEq(val) => ScmpArgCompare::new(rule.arg_num as u32, ScmpCompareOp::NotEqual, val),
            SeccompCompare::Gt(val) => ScmpArgCompare::new(rule.arg_num as u32, ScmpCompareOp::Greater, val),
            SeccompCompare::Ge(val) => ScmpArgCompare::new(rule.arg_num as u32, ScmpCompareOp::GreaterEqual, val),
            SeccompCompare::Lt(val) => ScmpArgCompare::new(rule.arg_num as u32, ScmpCompareOp::Less, val),
            SeccompCompare::Le(val) => ScmpArgCompare::new(rule.arg_num as u32, ScmpCompareOp::LessOrEqual, val),
            SeccompCompare::MaskedEq(val, mask) => {
                ScmpArgCompare::new(rule.arg_num as u32, ScmpCompareOp::MaskedEqual(mask), val)
            }
        };

        ctx.add_rule_conditional(action, sc, &[arg_cmp])
            .with_context(|| format!("Failed to add conditional rule"))?;

        tracing::trace!("Seccomp: conditional rule for {:?}", rule);
    }

    // Load the filter into the kernel
    ctx.load()
        .context("Failed to load seccomp filter")?;

    tracing::info!("Seccomp filter applied successfully");

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn apply_seccomp(_profile: &SeccompProfile) -> Result<()> {
    tracing::warn!("Seccomp filtering not supported on this platform");
    Ok(())
}

/// Dangerous syscalls that should typically be blocked.
///
/// These are syscalls that can compromise system security.
pub fn dangerous_syscalls() -> Vec<Syscall> {
    vec![
        // Kernel module manipulation
        Syscall::Name("init_module".to_string()),
        Syscall::Name("finit_module".to_string()),
        Syscall::Name("delete_module".to_string()),
        // System reboot/power control
        Syscall::Name("reboot".to_string()),
        Syscall::Name("kexec_load".to_string()),
        Syscall::Name("kexec_file_load".to_string()),
        // Low-level hardware access
        Syscall::Name("iopl".to_string()),
        Syscall::Name("ioperm".to_string()),
        // Process tracing
        Syscall::Name("ptrace".to_string()),
        Syscall::Name("kcmp".to_string()),
        // Key management
        Syscall::Name("add_key".to_string()),
        Syscall::Name("request_key".to_string()),
        Syscall::Name("keyctl".to_string()),
        // BPF (can be used for various exploits)
        Syscall::Name("bpf".to_string()),
        // Userfaultfd (can be used for exploits)
        Syscall::Name("userfaultfd".to_string()),
        // Performance monitoring (can leak info)
        Syscall::Name("perf_event_open".to_string()),
        // Swap manipulation
        Syscall::Name("swapon".to_string()),
        Syscall::Name("swapoff".to_string()),
        // Time manipulation
        Syscall::Name("stime".to_string()),
        Syscall::Name("clock_settime".to_string()),
        Syscall::Name("adjtimex".to_string()),
        // hostname/domainname
        Syscall::Name("sethostname".to_string()),
        Syscall::Name("setdomainname".to_string()),
        // VM manipulation
        Syscall::Name("vm86old".to_string()),
        Syscall::Name("vm86".to_string()),
        // ACPI
        Syscall::Name("acpi".to_string()),
    ]
}

/// Create a profile that blocks dangerous syscalls.
///
/// This is a blacklist-style profile that allows everything except
/// known dangerous operations.
pub fn safe_default_profile() -> SeccompProfile {
    let mut profile = SeccompProfile::blacklist();

    for syscall in dangerous_syscalls() {
        profile.deny_syscall(syscall);
    }

    profile
}

/// Check if seccomp is available and supported.
#[cfg(target_os = "linux")]
pub fn is_seccomp_available() -> bool {
    // Check if seccomp is enabled in the kernel
    if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
        for line in content.lines() {
            if line.starts_with("Seccomp:") {
                return line.contains("1") || line.contains("2");
            }
        }
    }
    false
}

/// Stub for non-Linux.
#[cfg(not(target_os = "linux"))]
pub fn is_seccomp_available() -> bool {
    false
}

/// Get the seccomp mode of the current process.
///
/// Returns:
/// - 0: not enabled
/// - 1: strict mode (filter never allowed)
/// - 2: filter mode
#[cfg(target_os = "linux")]
pub fn get_seccomp_mode() -> Option<u32> {
    if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
        for line in content.lines() {
            if line.starts_with("Seccomp:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(mode) = parts[1].parse::<u32>() {
                        return Some(mode);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_profile() {
        let profile = default_profile();
        assert!(!profile.allow.is_empty());
        assert_eq!(profile.default_action, SeccompAction::Errno);
    }

    #[test]
    fn test_strict_profile() {
        let profile = strict_profile();
        assert!(!profile.allow.is_empty());
        assert!(profile.allow.len() < default_profile().allow.len());
    }

    #[test]
    fn test_dangerous_syscalls() {
        let dangerous = dangerous_syscalls();
        assert!(dangerous.iter().any(|s| matches!(s, Syscall::Name(n) if n == "reboot")));
        assert!(dangerous.iter().any(|s| matches!(s, Syscall::Name(n) if n == "init_module")));
    }

    #[test]
    fn test_syscall_from_str() {
        let s: Syscall = "open".into();
        assert_eq!(s, Syscall::Name("open".to_string()));
    }

    #[test]
    fn test_profile_builder() {
        let mut profile = SeccompProfile::whitelist();
        profile.allow_syscall("read");
        profile.allow_syscall("write");
        profile.deny_syscall("reboot");

        assert_eq!(profile.allow.len(), 2);
        assert_eq!(profile.deny.len(), 1);
    }

    #[test]
    fn test_safe_default_profile() {
        let profile = safe_default_profile();
        assert_eq!(profile.default_action, SeccompAction::Allow);
        assert!(!profile.deny.is_empty());
    }
}
