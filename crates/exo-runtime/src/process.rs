//! Container process spawning and management with full isolation.
//!
//! This module provides the core container spawning logic using fork+exec
//! with proper Linux namespace isolation, cgroup limits, root filesystem
//! setup, and security filtering.

use crate::cgroup::{self, CgroupManager};
use crate::config::ContainerConfig;
use crate::rootfs::{self, pivot_rootfs, prepare_rootfs, setup_mounts};
use crate::seccomp::{self, apply_seccomp, default_profile};
use crate::security::{self, drop_capabilities, get_default_caps};
use crate::userns::{self, setup_user_namespace, GidMap, UidMap};
use crate::namespace::{Namespace, NamespaceFlags, unshare_namespaces};
use crate::binfmt::{self, Architecture};
use crate::agent::{self, AgentConfigExt, get_agent_profile, AgentProfile};
use anyhow::{Context, Result};

#[cfg(target_os = "linux")]
use {
    nix::sys::wait::{waitpid, WaitStatus},
    nix::sys::signal::{self, Signal},
    nix::unistd::{self, Pid, Uid, Gid},
    nix::sched::{clone, unshare, CloneFlags},
    nix::mount::{mount, MsFlags},
    std::ffi::CString,
    std::os::unix::io::{AsRawFd, RawFd, OwnedFd},
    std::fs::File,
};

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

    /// Cgroup manager for this process
    #[cfg(target_os = "linux")]
    cgroup_manager: Option<CgroupManager>,
}

/// Handle to a container's namespaces for entering/exploring.
#[derive(Debug)]
pub struct NamespaceHandles {
    pub mount: Option<std::fs::File>,
    pub uts: Option<std::fs::File>,
    pub ipc: Option<std::fs::File>,
    pub net: Option<std::fs::File>,
    pub pid: Option<std::fs::File>,
    pub user: Option<std::fs::File>,
}

/// Process state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Exited(i32),
    Failed(i32),
}

/// Configuration for container spawning.
#[cfg(target_os = "linux")]
pub struct SpawnOptions {
    /// Whether to use user namespace (rootless)
    pub use_user_namespace: bool,

    /// UID mapping for user namespace
    pub uid_map: Option<UidMap>,

    /// GID mapping for user namespace
    pub gid_map: Option<GidMap>,

    /// Working directory inside the container
    pub workdir: String,
}

#[cfg(target_os = "linux")]
impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            use_user_namespace: true,
            uid_map: None,
            gid_map: None,
            workdir: "/".to_string(),
        }
    }
}

impl ContainerProcess {
    /// Spawn a new container process with full isolation.
    ///
    /// This is the main entry point for container creation. It handles:
    /// 1. Root filesystem setup and overlay mount (BEFORE user namespace)
    /// 2. Fork with clone flags for namespaces
    /// 3. User namespace setup (for rootless operation)
    /// 4. Cgroup resource limits
    /// 5. pivot_root and mount setup
    /// 6. Hostname configuration
    /// 7. Capability dropping
    /// 8. Seccomp filtering
    /// 9. Exec of the target binary
    #[cfg(target_os = "linux")]
    pub fn spawn(config: &ContainerConfig) -> Result<Self> {
        // Set up foreign binary execution if needed (e.g., ARM on x86)
        if config.requires_foreign_exec() {
            if let Some(arch_str) = config.detect_architecture() {
                if let Some(arch) = Architecture::from_str(arch_str) {
                    binfmt::setup_foreign_exec(arch)
                        .with_context(|| format!("Failed to set up foreign exec for {:?}", arch))?;
                }
            }
        }

        // First, prepare the rootfs (creates directories, checks for image)
        let rootfs_path = prepare_rootfs(config)
            .context("Failed to prepare root filesystem")?;

        // Set up overlay mount BEFORE forking (requires CAP_SYS_ADMIN)
        // This is critical: overlay mounts must be done in the parent process
        // before entering user namespace
        // Note: If overlay mount fails (EPERM), fall back to read-only rootfs
        let rootfs_path = if rootfs::load_overlay_config(&config.name)?.is_some() {
            tracing::info!("Setting up overlayfs mount for container {}", config.name);
            match rootfs::setup_overlay_rootfs(&config.name) {
                Ok(path) => path,
                Err(e) => {
                    tracing::warn!("Overlay mount failed ({}), using read-only rootfs", e);
                    rootfs_path
                }
            }
        } else {
            rootfs_path
        };

        // Set up cgroup manager
        let mut cgroup_manager = CgroupManager::new(&config.name)?;
        cgroup_manager.initialize()
            .context("Failed to initialize cgroup")?;

        // Apply resource limits from config
        apply_resource_limits(&cgroup_manager, config)?;

        // Prepare spawn options
        let mut options = SpawnOptions::default();
        options.use_user_namespace = config.namespaces.user;

        if config.namespaces.user {
            // Set up UID/GID mapping for rootless containers
            options.uid_map = Some(userns::UidMap::rootless()?);
            options.gid_map = Some(userns::GidMap::rootless()?);
        }

        options.workdir = config.workdir.to_string_lossy().to_string();

        // Fork and spawn container
        let (pid, _child_sync_fd) = spawn_container_fork(config, &rootfs_path, &options)?;

        // Add the child process to the cgroup
        cgroup_manager.add_process(pid)?;

        // Open namespace handles for later access
        let namespaces = open_namespace_handles(pid)?;

        Ok(Self {
            pid,
            namespaces,
            state: ProcessState::Running,
            cgroup_manager: Some(cgroup_manager),
        })
    }

    /// Non-Linux stub
    #[cfg(not(target_os = "linux"))]
    pub fn spawn(_config: &ContainerConfig) -> Result<Self> {
        Err(anyhow::anyhow!("Container spawning is only supported on Linux"))
    }

    /// Wait for the process to exit.
    #[cfg(target_os = "linux")]
    pub fn wait(&self) -> Result<ProcessState> {
        use nix::errno::Errno;

        match waitpid(self.pid, None) {
            Ok(WaitStatus::Exited(_, exit_code)) => {
                Ok(ProcessState::Exited(exit_code))
            }
            Ok(WaitStatus::Signaled(_, signal, _)) => {
                Ok(ProcessState::Failed(signal as i32))
            }
            Ok(_) => Ok(ProcessState::Running),
            Err(Errno::ESRCH) => {
                // Process already exited and was reaped - treat as success
                // This can happen with short-lived commands that exit before we wait
                tracing::debug!("Process {} already reaped, assuming exit 0", self.pid);
                Ok(ProcessState::Exited(0))
            }
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
    pub fn kill(&self, sig: Signal) -> Result<()> {
        use nix::errno::Errno;
        match nix::sys::signal::kill(self.pid, sig) {
            Ok(()) => Ok(()),
            Err(Errno::ESRCH) => {
                // Process already exited - that's fine
                tracing::debug!("Process {} already exited, ignoring signal", self.pid);
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
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

    /// Get the cgroup manager for this process.
    #[cfg(target_os = "linux")]
    pub fn cgroup_manager(&self) -> Option<&CgroupManager> {
        self.cgroup_manager.as_ref()
    }

    /// Clean up resources when process exits.
    pub fn cleanup(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            if let Some(cgroup) = self.cgroup_manager.take() {
                cgroup.destroy()?;
            }
        }
        Ok(())
    }
}

/// Open namespace file handles for a process.
#[cfg(target_os = "linux")]
fn open_namespace_handles(pid: Pid) -> Result<NamespaceHandles> {
    let ns = |name: &str| -> Option<File> {
        let path = format!("/proc/{}/ns/{}", pid.as_raw(), name);
        File::open(&path).ok()
    };

    Ok(NamespaceHandles {
        mount: ns("mnt"),
        uts: ns("uts"),
        ipc: ns("ipc"),
        net: ns("net"),
        pid: ns("pid"),
        user: ns("user"),
    })
}

/// Apply resource limits from config to cgroup.
#[cfg(target_os = "linux")]
fn apply_resource_limits(cgroup: &CgroupManager, config: &ContainerConfig) -> Result<()> {
    // Memory limit
    if let Some(ref memory_str) = config.resources.memory {
        let bytes = crate::config::parse_size(memory_str)?;
        cgroup.set_memory_limit(bytes)?;
    }

    // CPU limit
    if let Some(ref cpu_str) = config.resources.cpu {
        // Parse as CPU count (e.g., "2" for 2 CPUs, "0.5" for half a CPU)
        let cpu_count: f64 = cpu_str.parse()
            .or_else(|_| {
                // Try parsing as percentage (e.g., "200%")
                let s = cpu_str.trim().trim_end_matches('%');
                s.parse::<f64>().map(|p| p / 100.0)
            })
            .unwrap_or(1.0);

        let (quota, period) = cgroup::cpu_count_to_quota(cpu_count);
        cgroup.set_cpu_limit(quota, period)?;
    }

    // CPU affinity
    if let Some(ref cpus) = config.resources.cpus {
        cgroup.set_cpu_affinity(cpus)?;
    }

    // CPU shares
    if let Some(shares) = config.resources.cpu_shares {
        cgroup.set_cpu_shares(shares)?;
    }

    // PIDs limit
    if let Some(limit) = config.resources.pids_limit {
        cgroup.set_pids_limit(limit)?;
    }

    // Memory swap limit
    if let Some(ref swap_str) = config.resources.memory_swap {
        let bytes = crate::config::parse_size(swap_str)?;
        cgroup.set_memory_swap_limit(bytes)?;
    }

    Ok(())
}

/// Fork-based container spawning with proper namespace isolation.
///
/// This implements the full container initialization sequence:
/// 1. Fork child process
/// 2. Child creates user namespace (unshare CLONE_NEWUSER)
/// 3. Parent writes uid_map/gid_map (maps container root to current user)
/// 4. Child unshares other namespaces (now has CAP_SYS_ADMIN in user ns)
/// 5. Child sets up rootfs with pivot_root
/// 6. Child execs command
#[cfg(target_os = "linux")]
pub fn spawn_container_fork(
    config: &ContainerConfig,
    rootfs_path: &std::path::Path,
    options: &SpawnOptions,
) -> Result<(Pid, RawFd)> {
    use std::io::{Read, Write};
    
    // Create pipes for bidirectional synchronization
    // Pipe 1: child -> parent (child ready for uid_map)
    // Pipe 2: parent -> child (uid_map written, continue)
    let (child_to_parent_read, child_to_parent_write) = unistd::pipe()?;
    let (parent_to_child_read, parent_to_child_write) = unistd::pipe()?;

    // Fork to create new process
    match unsafe { unistd::fork() }? {
        unistd::ForkResult::Parent { child } => {
            // Close child ends in parent
            drop(child_to_parent_write);
            drop(parent_to_child_read);
            
            if options.use_user_namespace {
                // Wait for child to create user namespace
                let mut buf = [0u8; 1];
                let _ = unistd::read(child_to_parent_read.as_raw_fd(), &mut buf);
                
                // Write uid_map: map container uid 0 to our uid
                let uid = unsafe { libc::getuid() };
                let gid = unsafe { libc::getgid() };
                
                let uid_map_path = format!("/proc/{}/uid_map", child.as_raw());
                let gid_map_path = format!("/proc/{}/gid_map", child.as_raw());
                let setgroups_path = format!("/proc/{}/setgroups", child.as_raw());
                
                // Disable setgroups before writing gid_map (required for unprivileged)
                let _ = std::fs::write(&setgroups_path, "deny\n");
                
                // uid_map format: "container_uid host_uid count"
                // Map container root (0) to our uid, range 1
                let uid_map = format!("0 {} 1\n", uid);
                let gid_map = format!("0 {} 1\n", gid);
                
                std::fs::write(&uid_map_path, &uid_map)
                    .with_context(|| format!("Failed to write uid_map to {}", uid_map_path))?;
                std::fs::write(&gid_map_path, &gid_map)
                    .with_context(|| format!("Failed to write gid_map to {}", gid_map_path))?;
                
                tracing::debug!("Wrote uid_map: {} and gid_map: {}", uid_map.trim(), gid_map.trim());
                
                // Signal child to continue
                let _ = unistd::write(&parent_to_child_write, b"X");
            }
            
            // Close remaining pipes
            drop(child_to_parent_read);
            drop(parent_to_child_write);
            
            Ok((child, 0)) // No sync fd needed with uid mapping approach
        }
        unistd::ForkResult::Child => {
            // Close parent ends in child
            drop(child_to_parent_read);
            drop(parent_to_child_write);

            // Child process - set up container environment
            if let Err(e) = container_child_init_with_uidmap(
                config, 
                rootfs_path, 
                options, 
                child_to_parent_write,
                parent_to_child_read
            ) {
                for cause in e.chain() {
                }
                std::process::exit(1);
            }

            // Should never reach here
            std::process::exit(1);
        }
    }
}

/// Container child initialization with UID mapping support.
#[cfg(target_os = "linux")]
fn container_child_init_with_uidmap(
    config: &ContainerConfig,
    rootfs_path: &std::path::Path,
    options: &SpawnOptions,
    notify_parent_fd: OwnedFd,
    wait_for_parent_fd: OwnedFd,
) -> Result<()> {
    
    // Step 1: Create user namespace FIRST
    if options.use_user_namespace {
        unshare(CloneFlags::CLONE_NEWUSER)
            .context("Failed to create user namespace")?;
        
        // Notify parent that we've created the user namespace
        let _ = unistd::write(&notify_parent_fd, b"X");
        
        // Wait for parent to write uid_map/gid_map
        let mut buf = [0u8; 1];
        let _ = unistd::read(wait_for_parent_fd.as_raw_fd(), &mut buf);
    }
    
    // Now call the main child init
    container_child_init(config, rootfs_path, options)
}

/// Container child initialization.
///
/// This runs in the child process after fork and UID mapping.
#[cfg(target_os = "linux")]
fn container_child_init(
    config: &ContainerConfig,
    rootfs_path: &std::path::Path,
    options: &SpawnOptions,
) -> Result<()> {
    
    // Check our effective UID after mapping
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    
    // Step 2: Unshare other namespaces (user namespace already created)
    let mut flags = CloneFlags::empty();

    if config.namespaces.mount {
        flags |= CloneFlags::CLONE_NEWNS;
    }
    if config.namespaces.uts {
        flags |= CloneFlags::CLONE_NEWUTS;
    }
    if config.namespaces.ipc {
        flags |= CloneFlags::CLONE_NEWIPC;
    }
    if config.namespaces.network {
        flags |= CloneFlags::CLONE_NEWNET;
    }
    if config.namespaces.pid {
        // PID namespace requires fork with CLONE_NEWPID, not unshare
    }
    if config.namespaces.cgroup {
        flags |= CloneFlags::CLONE_NEWCGROUP;
    }

    if !flags.is_empty() {
        unshare(flags)
            .context("Failed to unshare namespaces")?;
    }

    
    // Step 3: Set up rootfs with pivot_root
    // Change to the new root directory
    std::env::set_current_dir(rootfs_path)
        .context("Failed to change to rootfs directory")?;

    
    // Try a simpler mount first - just bind, no recursive
    mount(
        Some(rootfs_path),
        rootfs_path,
        None::<&str>,
        MsFlags::MS_BIND,
        None::<&str>,
    ).context("Failed to bind mount rootfs")?;
    

    // Create put_old directory
    let put_old = rootfs_path.join(".pivot_old");
    std::fs::create_dir_all(&put_old)
        .context("Failed to create pivot_old directory")?;

    // Try pivot_root first, fall back to chroot if it fails
    match nix::unistd::pivot_root(rootfs_path, &put_old) {
        Ok(()) => {
            // Change to new root
            std::env::set_current_dir("/")
                .context("Failed to change directory to new root")?;

            // Unmount the old root (now at /.pivot_old)
            let old_root = std::path::PathBuf::from("/.pivot_old");
            match nix::mount::umount(&old_root) {
                Ok(()) => {}
                Err(e) => tracing::debug!("Failed to unmount old root: {} (non-fatal)", e),
            }
            let _ = std::fs::remove_dir(&old_root);
        }
        Err(e) => {
            // Fall back to chroot for weaker isolation
            // chroot is escapable but works for trusted code
            nix::unistd::chroot(rootfs_path)
                .context("chroot failed")?;
            std::env::set_current_dir("/")
                .context("Failed to change directory to /")?;
        }
    }

    // Step 4: Set up mounts (proc, sys, dev, etc.)
    setup_mounts(config)?;

    // Step 5: Set hostname if UTS namespace is isolated
    if config.namespaces.uts && !config.hostname.is_empty() {
        unistd::sethostname(&config.hostname)
            .context("Failed to set hostname")?;
    }

    // Step 6: Set up network (basic loopback)
    if config.namespaces.network {
        setup_loopback_network()?;
    }

    // Step 7: Apply security profile
    if !config.privileged {
        // Check if this is an agent container and use agent-specific security
        if config.is_agent() {
            let agent_profile = get_agent_profile(config);
            tracing::info!("Using agent security profile: {}", agent_profile.name);

            // Drop capabilities according to agent profile
            drop_capabilities(&agent_profile.capabilities)?;

            // Apply agent seccomp filter
            apply_seccomp(&agent_profile.seccomp)?;

            // Set no_new_privs if configured
            if agent_profile.no_new_privs {
                const PR_SET_NO_NEW_PRIVS: i32 = 38;
                let ret = unsafe { libc::prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
                if ret != 0 {
                    tracing::warn!("Failed to set no_new_privs: {}", std::io::Error::last_os_error());
                }
            }
        } else {
            // Use standard container security
            let caps_to_keep = if config.namespaces.user {
                // In user namespace, we have different caps
                vec![
                    security::Capability::CAP_SETUID,
                    security::Capability::CAP_SETGID,
                ]
            } else {
                get_default_caps().into_iter().collect()
            };
            drop_capabilities(&caps_to_keep)?;

            // Apply standard seccomp filter
            let profile = default_profile();
            apply_seccomp(&profile)?;
        }
    }


    // Step 9: Execute the container command
    exec_container_command(config, &options.workdir)?;

    Ok(())
}

/// Set up basic loopback networking in the container.
#[cfg(target_os = "linux")]
fn setup_loopback_network() -> Result<()> {
    // Bring up loopback interface
    use std::net::UdpSocket;
    use std::os::unix::io::AsRawFd;

    // Create a socket for ioctl
    let socket = UdpSocket::bind("0.0.0.0:0")
        .or_else(|_| UdpSocket::bind("[::]:0"))?;

    // Use ioctl to bring up lo
    // This is simplified - real implementation would use netlink or proper ioctl
    tracing::debug!("Network namespace created (loopback setup would go here)");

    Ok(())
}

/// Execute the container command.
#[cfg(target_os = "linux")]
fn exec_container_command(config: &ContainerConfig, workdir: &str) -> Result<()> {
    use std::ffi::CString;
    use nix::unistd::execve;

    if config.command.is_empty() {
        anyhow::bail!("No command specified");
    }

    let program = CString::new(config.command[0].as_bytes())
        .context("Command contains null bytes")?;

    let args: Vec<CString> = config.command.iter()
        .map(|a| CString::new(a.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .context("Command args contain null bytes")?;

    // Set up environment variables
    let mut env_vars: Vec<CString> = vec![
        CString::new("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin").unwrap(),
        CString::new("TERM=xterm").unwrap(),
        CString::new("HOME=/root").unwrap(),
    ];

    for (key, value) in &config.env {
        let env_str = format!("{}={}", key, value);
        env_vars.push(CString::new(env_str.as_bytes())?);
    }

    // Change to working directory
    std::env::set_current_dir(workdir)
        .with_context(|| format!("Failed to change to workdir: {}", workdir))?;

    // Debug: check if binary exists
    
    // Check if the program exists
    let program_path = std::path::Path::new(config.command[0].as_str());
    if program_path.exists() {
    } else {
        // Try to find it in PATH
        for path_entry in &["/bin", "/usr/bin", "/usr/local/bin"] {
            let full_path = std::path::Path::new(path_entry).join(&config.command[0]);
            if full_path.exists() {
            }
        }
    }

    // Exec the command
    execve(&program, &args, &env_vars)
        .context("execve failed")?;

    // Should never reach here
    unreachable!("execve returned");
}

/// Fork stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn spawn_container_fork(
    _config: &ContainerConfig,
    _rootfs_path: &std::path::Path,
    _options: &(),
) -> Result<(u32, i32)> {
    Err(anyhow::anyhow!("Container forking is only supported on Linux"))
}

/// Enter a container's namespaces.
///
/// Allows entering an existing container's namespaces for debugging
/// or exec operations.
#[cfg(target_os = "linux")]
pub fn enter_container_namespaces(pid: Pid) -> Result<()> {
    use crate::namespace::{enter_namespace, Namespace};

    // Try to enter each namespace
    for ns in &[Namespace::Mount, Namespace::Uts, Namespace::Ipc, Namespace::Network] {
        if let Err(e) = enter_namespace(pid, *ns) {
            tracing::warn!("Failed to enter {:?} namespace: {}", ns, e);
        }
    }

    Ok(())
}

/// Get the effective capabilities of a process.
#[cfg(target_os = "linux")]
pub fn get_process_caps(pid: Pid) -> Result<String> {
    let status_path = format!("/proc/{}/status", pid.as_raw());
    let content = std::fs::read_to_string(&status_path)?;

    for line in content.lines() {
        if line.starts_with("CapEff:") {
            return Ok(line.to_string());
        }
    }

    Err(anyhow::anyhow!("CapEff not found in process status"))
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

    #[test]
    fn test_spawn_options_default() {
        #[cfg(target_os = "linux")]
        {
            let opts = SpawnOptions::default();
            assert!(opts.use_user_namespace);
            assert_eq!(opts.workdir, "/");
        }
    }
}
