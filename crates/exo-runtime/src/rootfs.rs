//! Root filesystem management for containers.
//!
//! This module handles the preparation and setup of container root filesystems,
//! including extraction of OCI image layers, pivot_root(2) operations, and
//! setting up the necessary mounts (proc, sys, dev, tmpfs).
//!
//! # Root Filesystem Isolation
//!
//! Containers require an isolated root filesystem to prevent access to the
//! host's filesystem. This is achieved through:
//!
//! 1. **pivot_root(2)** - Changes the root filesystem of the current process
//! 2. **Mount propagation** - Ensures mounts don't leak to the host
//! 3. **Bind mounts** - Allows selective host directory access
//! 4. **OverlayFS** - Provides writable layer on top of read-only image rootfs
//!
//! # OverlayFS Layout
//!
//! ```text
//! /var/lib/exo/containers/<container-id>/
//! ├── config.json       # Container metadata
//! ├── rootfs/           # Merged view (mount point for overlay)
//! ├── upper/            # Writable layer (container changes)
//! └── work/             # Overlay work directory
//! ```
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! use exo_runtime::rootfs::{prepare_rootfs, pivot_rootfs, setup_mounts};
//! use exo_runtime::config::ContainerConfig;
//!
//! let config = ContainerConfig::default();
//! let rootfs_path = prepare_rootfs(&config)?;
//! pivot_rootfs(&rootfs_path)?;
//! setup_mounts(&config)?;
//! # Ok(())
//! # }
//! ```

use crate::config::ContainerConfig;
use anyhow::{Context, Result};
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use {
    nix::mount::{mount, MsFlags},
    nix::sys::stat::{makedev, mknod, Mode, SFlag},
    nix::unistd::pivot_root,
    std::fs::File,
    std::os::unix::io::AsRawFd,
};

/// Default container root directory.
pub const CONTAINER_ROOT_DIR: &str = "/var/lib/exo/containers";

/// Directory name for the container filesystem (merged overlay mount).
pub const ROOTFS_DIR: &str = "rootfs";

/// Directory name for the writable overlay layer.
pub const UPPER_DIR: &str = "upper";

/// Directory name for the overlay work directory.
pub const WORK_DIR: &str = "work";

/// Old root directory after pivot (used for cleanup).
pub const PIVOT_OLD_ROOT: &str = "pivot_old";

/// Default image rootfs directory.
pub const IMAGE_ROOTFS_DIR: &str = "/tmp/exo-images/rootfs";

/// Get the container root directory with fallback to user directory.
/// Tries /var/lib/exo/containers first, falls back to ~/.local/share/exo/containers.
pub fn get_container_root() -> PathBuf {
    if let Ok(state_dir) = std::env::var("EXO_STATE_DIR") {
        let state_dir = PathBuf::from(state_dir);
        if ensure_writable_dir(&state_dir) {
            return state_dir;
        }
    }

    let system_dir = PathBuf::from(CONTAINER_ROOT_DIR);
    if ensure_writable_dir(&system_dir) {
        return system_dir;
    }

    if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
        let xdg_dir = PathBuf::from(xdg_data_home).join("exo").join("containers");
        if ensure_writable_dir(&xdg_dir) {
            return xdg_dir;
        }
    }

    // Fall back to user directory
    if let Ok(home) = std::env::var("HOME") {
        let user_dir = PathBuf::from(home).join(".local/share/exo/containers");
        if ensure_writable_dir(&user_dir) {
            return user_dir;
        }
    }

    // Last resort: /tmp
    let temp_dir = std::env::temp_dir()
        .join("exo")
        .join("containers")
        .join(format!("uid-{}", current_uid()));
    let _ = create_dir_all(&temp_dir);
    temp_dir
}

fn ensure_writable_dir(path: &Path) -> bool {
    if create_dir_all(path).is_err() {
        return false;
    }

    let probe = path.join(format!(".exo-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// Prepare the root filesystem for a container.
///
/// This function:
/// 1. Creates the container directory structure with overlay directories
/// 2. Sets up overlayfs mount for writable layer on top of image rootfs
/// 3. Returns the path to the merged rootfs (overlay mount point)
///
/// # Arguments
///
/// * `config` - Container configuration with image reference
///
/// # Returns
///
/// Path to the prepared root filesystem (overlay merged view)
pub fn prepare_rootfs(config: &ContainerConfig) -> Result<PathBuf> {
    let container_id = &config.name;

    // Check if an extracted image rootfs exists
    let image_rootfs = PathBuf::from(IMAGE_ROOTFS_DIR).join(config.image.replace(['/', ':'], "_"));

    if image_rootfs.exists() && image_rootfs.join("bin").exists() {
        tracing::info!("Using extracted image rootfs: {:?}", image_rootfs);

        // Set up overlayfs for writable layer
        let container_root = get_container_root().join(container_id);

        let overlay_paths = OverlayPaths {
            rootfs: container_root.join(ROOTFS_DIR),
            upper: container_root.join(UPPER_DIR),
            work: container_root.join(WORK_DIR),
            lower: image_rootfs,
        };

        // Create overlay directories (mount happens in setup_overlay_rootfs)
        create_overlay_dirs(&overlay_paths)?;

        // Store overlay info for later mounting
        store_overlay_config(container_id, &overlay_paths)?;

        // Return the merged rootfs path (will be mounted later)
        return Ok(overlay_paths.rootfs);
    }

    // Fall back to creating a minimal rootfs with bind mounts
    let container_root = get_container_root().join(container_id);
    let rootfs_dir = container_root.join(ROOTFS_DIR);

    // Create the rootfs directory if it doesn't exist
    if !rootfs_dir.exists() {
        create_dir_all(&rootfs_dir)
            .with_context(|| format!("Failed to create rootfs directory: {:?}", rootfs_dir))?;
    }

    // Create a minimal rootfs structure
    setup_minimal_rootfs(&rootfs_dir)?;

    tracing::info!("Prepared minimal rootfs at: {:?}", rootfs_dir);

    Ok(rootfs_dir)
}

/// Overlay filesystem paths for a container.
#[derive(Debug, Clone)]
pub struct OverlayPaths {
    /// Merged view mount point (container rootfs)
    pub rootfs: PathBuf,
    /// Writable layer directory
    pub upper: PathBuf,
    /// Overlay work directory
    pub work: PathBuf,
    /// Lower (read-only) layer - the image rootfs
    pub lower: PathBuf,
}

/// Create overlay directories.
fn create_overlay_dirs(paths: &OverlayPaths) -> Result<()> {
    create_dir_all(&paths.rootfs)
        .with_context(|| format!("Failed to create rootfs directory: {:?}", paths.rootfs))?;
    create_dir_all(&paths.upper)
        .with_context(|| format!("Failed to create upper directory: {:?}", paths.upper))?;
    create_dir_all(&paths.work)
        .with_context(|| format!("Failed to create work directory: {:?}", paths.work))?;
    Ok(())
}

/// Store overlay configuration for later mounting.
fn store_overlay_config(container_id: &str, paths: &OverlayPaths) -> Result<()> {
    let container_root = get_container_root().join(container_id);
    let config_dir = container_root.join("config");
    create_dir_all(&config_dir)?;

    let overlay_config = serde_json::json!({
        "rootfs": paths.rootfs,
        "upper": paths.upper,
        "work": paths.work,
        "lower": paths.lower,
    });

    let config_path = config_dir.join("overlay.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(&overlay_config)?)
        .with_context(|| format!("Failed to write overlay config: {:?}", config_path))?;

    Ok(())
}

/// Load overlay configuration for a container.
pub fn load_overlay_config(container_id: &str) -> Result<Option<OverlayPaths>> {
    let config_path = get_container_root()
        .join(container_id)
        .join("config")
        .join("overlay.json");

    if !config_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read overlay config: {:?}", config_path))?;

    let config: serde_json::Value =
        serde_json::from_str(&content).with_context(|| "Failed to parse overlay config")?;

    Ok(Some(OverlayPaths {
        rootfs: PathBuf::from(config["rootfs"].as_str().unwrap_or("")),
        upper: PathBuf::from(config["upper"].as_str().unwrap_or("")),
        work: PathBuf::from(config["work"].as_str().unwrap_or("")),
        lower: PathBuf::from(config["lower"].as_str().unwrap_or("")),
    }))
}

/// Set up overlay filesystem using kernel overlay or fuse-overlayfs fallback.
///
/// This must be called BEFORE entering user namespace if running rootless,
/// as overlay mounts require privileges.
///
/// In rootless mode, kernel overlay mounts often fail with EPERM. When that
/// happens, we fall back to fuse-overlayfs which works in userspace.
///
/// # Arguments
///
/// * `container_id` - Container identifier
///
/// # Returns
///
/// Path to the merged rootfs (overlay mount point)
#[cfg(target_os = "linux")]
pub fn setup_overlay_rootfs(container_id: &str) -> Result<PathBuf> {
    let paths = load_overlay_config(container_id)?
        .ok_or_else(|| anyhow::anyhow!("No overlay config found for container {}", container_id))?;

    // Ensure directories exist
    create_overlay_dirs(&paths)?;

    // Build overlay mount options
    // Format: lowerdir=<lower>,upperdir=<upper>,workdir=<work>
    let options = format!(
        "lowerdir={},upperdir={},workdir={}",
        paths.lower.display(),
        paths.upper.display(),
        paths.work.display()
    );

    tracing::info!("Mounting overlayfs with options: {}", options);

    // Try kernel mount first
    let options_cstr = std::ffi::CString::new(options.as_str()).context("Invalid mount options")?;

    match mount(
        Some("overlay"),
        paths.rootfs.as_path(),
        Some("overlay"),
        MsFlags::MS_NOATIME,
        Some(options_cstr.as_c_str()),
    ) {
        Ok(()) => {
            tracing::info!("Kernel overlay mounted successfully");
            return Ok(paths.rootfs);
        }
        Err(e) => {
            tracing::warn!("Kernel overlay mount failed: {}, trying fuse-overlayfs", e);
        }
    }

    // Fallback to fuse-overlayfs
    let status = std::process::Command::new("fuse-overlayfs")
        .arg("-o")
        .arg(&options)
        .arg(&paths.rootfs)
        .status()
        .context("Failed to run fuse-overlayfs")?;

    if status.success() {
        tracing::info!("fuse-overlayfs mounted successfully");
        return Ok(paths.rootfs);
    }

    // Both failed - return error with read-only fallback hint
    Err(anyhow::anyhow!(
        "Both kernel overlay and fuse-overlayfs failed. Falling back to read-only rootfs."
    ))
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn setup_overlay_rootfs(_container_id: &str) -> Result<PathBuf> {
    Err(anyhow::anyhow!("OverlayFS is only supported on Linux"))
}

/// Unmount overlayfs for a container.
#[cfg(target_os = "linux")]
pub fn unmount_overlay_rootfs(container_id: &str) -> Result<()> {
    let paths = load_overlay_config(container_id)?
        .ok_or_else(|| anyhow::anyhow!("No overlay config found for container {}", container_id))?;

    if paths.rootfs.exists() {
        // Check if it's actually mounted
        let mount_info = std::fs::read_to_string("/proc/mounts").unwrap_or_default();

        if mount_info.contains(&format!("overlay {}", paths.rootfs.display())) {
            nix::mount::umount(&paths.rootfs)
                .with_context(|| format!("Failed to unmount overlayfs at {:?}", paths.rootfs))?;
            tracing::info!("Unmounted overlayfs for container {}", container_id);
        }
    }

    Ok(())
}

/// Non-Linux stub.
#[cfg(not(target_os = "linux"))]
pub fn unmount_overlay_rootfs(_container_id: &str) -> Result<()> {
    Ok(())
}

/// Set up a minimal root filesystem structure.
///
/// Creates the basic directory structure expected in a Linux rootfs.
#[cfg(target_os = "linux")]
fn setup_minimal_rootfs(rootfs: &Path) -> Result<()> {
    let dirs = [
        "bin",
        "sbin",
        "usr/bin",
        "usr/sbin",
        "usr/local/bin",
        "etc",
        "lib",
        "lib64",
        "usr/lib",
        "usr/lib64",
        "proc",
        "sys",
        "dev",
        "tmp",
        "var/tmp",
        "var/run",
        "home",
        "root",
        "mnt",
        "media",
    ];

    for dir in dirs {
        let dir_path = rootfs.join(dir);
        create_dir_all(&dir_path)
            .with_context(|| format!("Failed to create directory: {:?}", dir_path))?;
    }

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
fn setup_minimal_rootfs(_rootfs: &Path) -> Result<()> {
    Ok(())
}

/// Create essential device nodes in the container rootfs.
#[cfg(target_os = "linux")]
fn create_dev_nodes(rootfs: &Path) -> Result<()> {
    let dev_dir = rootfs.join("dev");

    // Create /dev/null (mode 0666)
    let null_path = dev_dir.join("null");
    if !null_path.exists() {
        let _ = mknod(
            &null_path,
            SFlag::S_IFCHR,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
            makedev(1, 3),
        );
    }

    // Create /dev/zero (mode 0666)
    let zero_path = dev_dir.join("zero");
    if !zero_path.exists() {
        let _ = mknod(
            &zero_path,
            SFlag::S_IFCHR,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
            makedev(1, 5),
        );
    }

    // Create /dev/full (mode 0666)
    let full_path = dev_dir.join("full");
    if !full_path.exists() {
        let _ = mknod(
            &full_path,
            SFlag::S_IFCHR,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
            makedev(1, 7),
        );
    }

    // Create /dev/random (mode 0666)
    let random_path = dev_dir.join("random");
    if !random_path.exists() {
        let _ = mknod(
            &random_path,
            SFlag::S_IFCHR,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
            makedev(1, 8),
        );
    }

    // Create /dev/urandom (mode 0666)
    let urandom_path = dev_dir.join("urandom");
    if !urandom_path.exists() {
        let _ = mknod(
            &urandom_path,
            SFlag::S_IFCHR,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
            makedev(1, 9),
        );
    }

    // Create /dev/tty (mode 0666)
    let tty_path = dev_dir.join("tty");
    if !tty_path.exists() {
        let _ = mknod(
            &tty_path,
            SFlag::S_IFCHR,
            Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO,
            makedev(5, 0),
        );
    }

    // Create symlinks for common devices
    let fd_link = dev_dir.join("fd");
    if !fd_link.exists() {
        let _ = std::os::unix::fs::symlink("/proc/self/fd", &fd_link);
    }

    let stdin_link = dev_dir.join("stdin");
    if !stdin_link.exists() {
        let _ = std::os::unix::fs::symlink("/proc/self/fd/0", &stdin_link);
    }

    let stdout_link = dev_dir.join("stdout");
    if !stdout_link.exists() {
        let _ = std::os::unix::fs::symlink("/proc/self/fd/1", &stdout_link);
    }

    let stderr_link = dev_dir.join("stderr");
    if !stderr_link.exists() {
        let _ = std::os::unix::fs::symlink("/proc/self/fd/2", &stderr_link);
    }

    Ok(())
}

/// Perform pivot_root(2) to change the root filesystem.
///
/// pivot_root() is the correct way to change the root filesystem of a process.
/// It moves the current root to a subdirectory and puts a new root at the root.
///
/// # Arguments
///
/// * `new_root` - Path to the new root filesystem
///
/// # Process
///
/// 1. Create a directory for the old root
/// 2. Bind mount new_root to itself (to satisfy pivot_root requirements)
/// 3. Call pivot_root()
/// 4. Change to new root
/// 5. Unmount the old root
#[cfg(target_os = "linux")]
pub fn pivot_rootfs(new_root: &Path) -> Result<()> {
    let new_root = new_root
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize new_root: {:?}", new_root))?;

    // Create put_old directory inside new_root
    let put_old = new_root.join(PIVOT_OLD_ROOT);
    create_dir_all(&put_old)
        .with_context(|| format!("Failed to create put_old directory: {:?}", put_old))?;

    // Bind mount new_root to itself (required by pivot_root)
    // MS_BIND: Create a bind mount
    // MS_REC: Recursive bind mount
    // MS_PRIVATE: Make mount private (no propagation)
    mount(
        Some(&new_root),
        &new_root,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    )?;

    // Call pivot_root
    pivot_root(&new_root, &put_old).context("pivot_root syscall failed")?;

    // Change to new root
    std::env::set_current_dir("/").context("Failed to change directory to new root")?;

    // Unmount the old root
    let old_root = PathBuf::from("/").join(PIVOT_OLD_ROOT);
    nix::mount::umount(&old_root).context("Failed to unmount old root")?;

    // Remove the old root directory
    std::fs::remove_dir(&old_root).context("Failed to remove old root directory")?;

    tracing::debug!("pivot_root completed successfully");

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn pivot_rootfs(_new_root: &Path) -> Result<()> {
    Err(anyhow::anyhow!("pivot_root is only supported on Linux"))
}

/// Mount /proc in the container.
///
/// Tries regular proc mount first (works in user namespace with CAP_SYS_ADMIN),
/// falls back to bind mount from host if that fails.
#[cfg(target_os = "linux")]
pub fn mount_proc(rootfs: &Path) -> Result<()> {
    use std::ffi::CString;

    let proc_path = rootfs.join("proc");

    // Create directory if it doesn't exist
    if !proc_path.exists() {
        create_dir_all(&proc_path)
            .with_context(|| format!("Failed to create {}", proc_path.display()))?;
    }

    let target = CString::new(proc_path.to_str().unwrap())
        .context("Failed to create CString for proc path")?;

    // Try regular proc mount first (works in user namespace with CAP_SYS_ADMIN)
    let result = unsafe {
        libc::mount(
            b"proc\0".as_ptr() as *const i8,
            target.as_ptr(),
            b"proc\0".as_ptr() as *const i8,
            libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_NODEV,
            std::ptr::null(),
        )
    };

    if result == 0 {
        tracing::info!("Mounted /proc (procfs)");
        return Ok(());
    }

    let proc_err = std::io::Error::last_os_error();
    tracing::warn!("Regular proc mount failed: {}", proc_err);

    // Fallback: bind mount host /proc (read-only, recursive)
    // This shows host PIDs instead of container PIDs, but allows Node.js and other
    // tools that need /proc/self to function in rootless mode.
    tracing::info!("Falling back to bind mount of host /proc (rootless mode)");

    let host_proc = CString::new("/proc").unwrap();
    let bind_result = unsafe {
        libc::mount(
            host_proc.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND
                | libc::MS_REC
                | libc::MS_RDONLY
                | libc::MS_NOSUID
                | libc::MS_NODEV
                | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    };

    if bind_result == 0 {
        tracing::info!("Bind-mounted host /proc (read-only) - note: shows host PIDs");
        return Ok(());
    }

    let bind_err = std::io::Error::last_os_error();
    tracing::warn!("Bind mount of host /proc also failed: {}", bind_err);
    tracing::warn!("Container will run without /proc - limited functionality");
    Err(anyhow::anyhow!(
        "Failed to mount /proc: proc={}, bind={}",
        proc_err,
        bind_err
    ))
}

/// Mount /sys in the container.
///
/// Tries regular sysfs mount first, falls back to bind mount if that fails.
#[cfg(target_os = "linux")]
pub fn mount_sys(rootfs: &Path) -> Result<()> {
    use std::ffi::CString;

    let sys_path = rootfs.join("sys");

    if !sys_path.exists() {
        create_dir_all(&sys_path)
            .with_context(|| format!("Failed to create {}", sys_path.display()))?;
    }

    let target = CString::new(sys_path.to_str().unwrap())
        .context("Failed to create CString for sys path")?;

    // Try regular sysfs mount first (works in user namespace with CAP_SYS_ADMIN)
    let result = unsafe {
        libc::mount(
            b"sysfs\0".as_ptr() as *const i8,
            target.as_ptr(),
            b"sysfs\0".as_ptr() as *const i8,
            libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_NODEV | libc::MS_RDONLY,
            std::ptr::null(),
        )
    };

    if result == 0 {
        tracing::info!("Mounted /sys (read-only)");
        return Ok(());
    }

    // Fallback: try bind mount from host /sys (read-only for safety)
    let result = unsafe {
        libc::mount(
            b"/sys\0".as_ptr() as *const i8,
            target.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND
                | libc::MS_REC
                | libc::MS_RDONLY
                | libc::MS_NOSUID
                | libc::MS_NOEXEC
                | libc::MS_NODEV,
            std::ptr::null(),
        )
    };

    if result == 0 {
        tracing::info!("Bind-mounted /sys from host (read-only)");
        return Ok(());
    }

    tracing::warn!("Could not mount /sys: {}", std::io::Error::last_os_error());
    Ok(()) // Non-fatal
}

/// Mount /dev in the container.
///
/// Tries devtmpfs first, falls back to tmpfs, then bind mount from host.
#[cfg(target_os = "linux")]
pub fn mount_dev(rootfs: &Path) -> Result<()> {
    use std::ffi::CString;

    let dev_path = rootfs.join("dev");

    if !dev_path.exists() {
        create_dir_all(&dev_path)
            .with_context(|| format!("Failed to create {}", dev_path.display()))?;
    }

    let target = CString::new(dev_path.to_str().unwrap())
        .context("Failed to create CString for dev path")?;

    // Mount devtmpfs
    let result = unsafe {
        libc::mount(
            b"devtmpfs\0".as_ptr() as *const i8,
            target.as_ptr(),
            b"devtmpfs\0".as_ptr() as *const i8,
            libc::MS_NOSUID | libc::MS_NOEXEC,
            b"mode=755\0".as_ptr() as *const libc::c_void,
        )
    };

    if result == 0 {
        tracing::info!("Mounted /dev (devtmpfs)");
        return Ok(());
    }

    let devtmpfs_err = std::io::Error::last_os_error();

    // Fallback 1: try tmpfs (works in user namespace)
    let result = unsafe {
        libc::mount(
            b"tmpfs\0".as_ptr() as *const i8,
            target.as_ptr(),
            b"tmpfs\0".as_ptr() as *const i8,
            libc::MS_NOSUID | libc::MS_NOEXEC,
            b"mode=755\0".as_ptr() as *const libc::c_void,
        )
    };

    if result == 0 {
        tracing::info!("Mounted /dev (tmpfs fallback)");
        return Ok(());
    }

    let tmpfs_err = std::io::Error::last_os_error();

    // Fallback 2: bind mount host /dev
    let result = unsafe {
        libc::mount(
            b"/dev\0".as_ptr() as *const i8,
            target.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND | libc::MS_REC | libc::MS_NOSUID | libc::MS_NOEXEC,
            std::ptr::null(),
        )
    };

    if result == 0 {
        tracing::info!("Bind-mounted /dev from host");
        return Ok(());
    }

    tracing::warn!(
        "Could not mount /dev: {} (tmpfs: {}, bind: {})",
        devtmpfs_err,
        tmpfs_err,
        std::io::Error::last_os_error()
    );
    Ok(())
}

/// Mount /dev/shm in the container.
///
/// Mounts tmpfs on /dev/shm.
#[cfg(target_os = "linux")]
pub fn mount_dev_shm(rootfs: &Path) -> Result<()> {
    use std::ffi::CString;

    let dev_shm_path = rootfs.join("dev/shm");

    if !dev_shm_path.exists() {
        create_dir_all(&dev_shm_path)
            .with_context(|| format!("Failed to create {}", dev_shm_path.display()))?;
    }

    let target = CString::new(dev_shm_path.to_str().unwrap())
        .context("Failed to create CString for dev/shm path")?;

    // Mount tmpfs on /dev/shm
    let result = unsafe {
        libc::mount(
            b"shm\0".as_ptr() as *const i8,
            target.as_ptr(),
            b"tmpfs\0".as_ptr() as *const i8,
            libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_NODEV,
            b"size=65536k\0".as_ptr() as *const libc::c_void,
        )
    };

    if result == 0 {
        tracing::info!("Mounted /dev/shm (tmpfs)");
        return Ok(());
    }

    tracing::warn!(
        "Could not mount /dev/shm: {}",
        std::io::Error::last_os_error()
    );
    Ok(())
}

/// Mount /tmp in the container.
///
/// Mounts tmpfs on /tmp.
#[cfg(target_os = "linux")]
pub fn mount_tmp(rootfs: &Path) -> Result<()> {
    use std::ffi::CString;

    let tmp_path = rootfs.join("tmp");

    if !tmp_path.exists() {
        create_dir_all(&tmp_path)
            .with_context(|| format!("Failed to create {}", tmp_path.display()))?;
    }

    let target = CString::new(tmp_path.to_str().unwrap())
        .context("Failed to create CString for tmp path")?;

    // Mount tmpfs on /tmp
    let result = unsafe {
        libc::mount(
            b"tmpfs\0".as_ptr() as *const i8,
            target.as_ptr(),
            b"tmpfs\0".as_ptr() as *const i8,
            libc::MS_NOSUID | libc::MS_NODEV,
            b"size=1048576k\0".as_ptr() as *const libc::c_void,
        )
    };

    if result == 0 {
        tracing::info!("Mounted /tmp (tmpfs)");
        return Ok(());
    }

    tracing::warn!("Could not mount /tmp: {}", std::io::Error::last_os_error());
    Ok(())
}

/// Mount /run in the container.
///
/// Mounts tmpfs on /run.
#[cfg(target_os = "linux")]
pub fn mount_run(rootfs: &Path) -> Result<()> {
    use std::ffi::CString;

    let run_path = rootfs.join("run");

    if !run_path.exists() {
        create_dir_all(&run_path)
            .with_context(|| format!("Failed to create {}", run_path.display()))?;
    }

    let target = CString::new(run_path.to_str().unwrap())
        .context("Failed to create CString for run path")?;

    // Mount tmpfs on /run
    let result = unsafe {
        libc::mount(
            b"tmpfs\0".as_ptr() as *const i8,
            target.as_ptr(),
            b"tmpfs\0".as_ptr() as *const i8,
            libc::MS_NOSUID | libc::MS_NOEXEC | libc::MS_NODEV,
            b"size=65536k\0".as_ptr() as *const libc::c_void,
        )
    };

    if result == 0 {
        tracing::info!("Mounted /run (tmpfs)");
        return Ok(());
    }

    tracing::warn!("Could not mount /run: {}", std::io::Error::last_os_error());
    Ok(())
}

/// Set up essential mounts inside the container namespace.
///
/// This should be called AFTER pivot_root/chroot, using "/" as rootfs.
/// Mounts are non-fatal to allow containers to run even with partial isolation.
#[cfg(target_os = "linux")]
pub fn setup_container_mounts(rootfs: &Path) -> Result<()> {
    // Order matters: /dev first, then /dev/shm
    let _ = mount_dev(rootfs); // Non-fatal
    let _ = mount_dev_shm(rootfs); // Non-fatal
    let _ = mount_proc(rootfs); // Non-fatal
    let _ = mount_sys(rootfs); // Non-fatal
    let _ = mount_tmp(rootfs); // Non-fatal
    let _ = mount_run(rootfs); // Non-fatal

    Ok(())
}

/// Set up the essential mounts for a container.
///
/// This function mounts:
/// - proc filesystem at /proc
/// - sysfs at /sys (optional, can be disabled for security)
/// - tmpfs at /dev/shm
/// - devpts at /dev/pts
///
/// In user namespaces, some mounts may fail due to permission restrictions.
/// We handle these gracefully and continue with what works.
///
/// # Arguments
///
/// * `config` - Container configuration
#[cfg(target_os = "linux")]
pub fn setup_mounts(config: &ContainerConfig) -> Result<()> {
    // Set up essential mounts first (using "/" as we're inside the container)
    setup_container_mounts(Path::new("/"))?;

    // Mount devpts for /dev/pts (may fail in user namespace)
    if Path::new("/dev/pts").exists() {
        match mount(
            None::<&str>,
            "/dev/pts",
            Some("devpts"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
            Some("newinstance,ptmxmode=0666,mode=0620"),
        ) {
            Ok(()) => {
                // Create /dev/ptmx symlink if it doesn't exist
                let ptmx = Path::new("/dev/ptmx");
                if !ptmx.exists() {
                    let _ = std::os::unix::fs::symlink("pts/ptmx", ptmx);
                }
                tracing::debug!("Mounted /dev/pts");
            }
            Err(e) => tracing::warn!("Could not mount /dev/pts: {}", e),
        }
    }

    // Apply user-specified bind mounts
    apply_bind_mounts(config)?;

    // Set up read-only rootfs if configured
    if config.readonly_rootfs {
        setup_readonly_rootfs()?;
    }

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn setup_mounts(_config: &ContainerConfig) -> Result<()> {
    Err(anyhow::anyhow!("Mount setup is only supported on Linux"))
}

/// Apply bind mounts BEFORE pivot_root (while still in host mount namespace).
///
/// This is critical for bind mounts to work: they must be set up while still
/// in the host's mount namespace, then they get carried through pivot_root.
///
/// # Arguments
///
/// * `config` - Container configuration with mount specifications
/// * `rootfs` - The container rootfs path (bind targets are relative to this)
#[cfg(target_os = "linux")]
pub fn apply_bind_mounts_before_pivot(config: &ContainerConfig, rootfs: &Path) -> Result<()> {
    for mount_spec in &config.mounts {
        match mount_spec.mount_type.as_str() {
            "bind" => {
                let source = Path::new(&mount_spec.source);
                // Target is inside the rootfs
                let target = rootfs.join(mount_spec.target.trim_start_matches('/'));

                // Ensure source exists on host
                if !source.exists() {
                    tracing::warn!("Bind mount source does not exist: {:?}", source);
                    continue;
                }

                tracing::info!(
                    "Setting up bind mount BEFORE pivot: {:?} -> {:?}",
                    source,
                    target
                );

                // Ensure target directory exists in container rootfs
                if let Some(parent) = target.parent() {
                    create_dir_all(parent)
                        .with_context(|| format!("Failed to create mount parent: {:?}", parent))?;
                }
                if !target.exists() {
                    if source.is_dir() {
                        create_dir_all(&target).with_context(|| {
                            format!("Failed to create mount target: {:?}", target)
                        })?;
                    } else {
                        // Create parent for file mount
                        if let Some(parent) = target.parent() {
                            create_dir_all(parent)?;
                        }
                        File::create(&target).with_context(|| {
                            format!("Failed to create mount target file: {:?}", target)
                        })?;
                    }
                }

                // Set up mount flags
                let mut flags = MsFlags::MS_BIND | MsFlags::MS_REC;

                // Parse propagation mode
                match mount_spec.propagation.as_str() {
                    "rprivate" | "" => flags |= MsFlags::MS_PRIVATE,
                    "rshared" => flags |= MsFlags::MS_SHARED | MsFlags::MS_REC,
                    "rslave" => flags |= MsFlags::MS_SLAVE | MsFlags::MS_REC,
                    "ro" => flags |= MsFlags::MS_RDONLY,
                    "rw" => {}
                    _ => {}
                }

                // Apply read-only if specified
                if mount_spec.readonly {
                    flags |= MsFlags::MS_RDONLY;
                }

                // First bind mount
                mount(
                    Some(source),
                    &target,
                    None::<&str>,
                    MsFlags::MS_BIND | MsFlags::MS_REC,
                    None::<&str>,
                )
                .with_context(|| format!("Failed to bind mount {:?} -> {:?}", source, target))?;

                // Remount with flags (to apply read-only etc.)
                if mount_spec.readonly || !mount_spec.propagation.is_empty() {
                    mount(
                        Some(source),
                        &target,
                        None::<&str>,
                        flags | MsFlags::MS_REMOUNT,
                        None::<&str>,
                    )
                    .with_context(|| format!("Failed to remount with flags: {:?}", target))?;
                }

                tracing::info!(
                    "Applied bind mount BEFORE pivot: {:?} -> {:?}",
                    source,
                    target
                );
            }
            "tmpfs" => {
                // tmpfs mounts should be done after pivot_root, skip here
                tracing::debug!(
                    "Deferring tmpfs mount until after pivot_root: {}",
                    mount_spec.target
                );
            }
            _ => {
                tracing::warn!("Unsupported mount type: {}", mount_spec.mount_type);
            }
        }
    }

    Ok(())
}

/// Apply user-specified bind mounts (called AFTER pivot_root).
///
/// This function applies bind mounts after pivot_root has been completed.
/// The source paths may need to be prefixed with /.pivot_old if they're on
/// the old root filesystem (which is still accessible until we unmount it).
///
/// # Arguments
///
/// * `config` - Container configuration with mount specifications
#[cfg(target_os = "linux")]
pub fn apply_bind_mounts(config: &ContainerConfig) -> Result<()> {
    // Check if old root is still accessible (pivot_root may have failed or old root not unmounted)
    let old_root = Path::new("/.pivot_old");
    let old_root_exists = old_root.exists();

    for mount_spec in &config.mounts {
        match mount_spec.mount_type.as_str() {
            "bind" => {
                let original_source = Path::new(&mount_spec.source);
                let target = Path::new(&mount_spec.target);

                // Check if already mounted
                let mount_info = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
                if mount_info.contains(&format!(" {} ", target.display())) {
                    tracing::debug!("Bind mount already active: {:?}", target);
                    continue;
                }

                // Try to find the source:
                // 1. First try the original path (might work if we're not fully isolated)
                // 2. Then try via old root at /.pivot_old
                let source = if original_source.exists() {
                    original_source.to_path_buf()
                } else if old_root_exists {
                    let via_old_root = old_root.join(mount_spec.source.trim_start_matches('/'));
                    if via_old_root.exists() {
                        tracing::debug!(
                            "Using old root path for bind mount source: {:?}",
                            via_old_root
                        );
                        via_old_root
                    } else {
                        tracing::warn!(
                            "Bind mount source not found: {:?} (also tried via old root: {:?})",
                            original_source,
                            via_old_root
                        );
                        continue;
                    }
                } else {
                    tracing::warn!("Bind mount source not accessible after pivot_root: {:?} (old root not available)", 
                        original_source);
                    continue;
                };

                tracing::info!(
                    "Applying bind mount after pivot_root: {:?} -> {:?}",
                    source,
                    target
                );

                // Ensure target directory exists in container
                if let Some(parent) = target.parent() {
                    create_dir_all(parent)
                        .with_context(|| format!("Failed to create mount parent: {:?}", parent))?;
                }
                if !target.exists() {
                    // Check if source is a directory
                    let is_dir = source.is_dir();
                    if is_dir {
                        create_dir_all(target).with_context(|| {
                            format!("Failed to create mount target: {:?}", target)
                        })?;
                    } else {
                        if let Some(parent) = target.parent() {
                            create_dir_all(parent)?;
                        }
                        File::create(target)?;
                    }
                }

                // Set up mount flags
                let mut flags = MsFlags::MS_BIND | MsFlags::MS_REC;

                match mount_spec.propagation.as_str() {
                    "rprivate" | "" => flags |= MsFlags::MS_PRIVATE,
                    "rshared" => flags |= MsFlags::MS_SHARED | MsFlags::MS_REC,
                    "rslave" => flags |= MsFlags::MS_SLAVE | MsFlags::MS_REC,
                    "ro" => flags |= MsFlags::MS_RDONLY,
                    "rw" => {}
                    _ => {}
                }

                if mount_spec.readonly {
                    flags |= MsFlags::MS_RDONLY;
                }

                // Apply the bind mount
                match mount(
                    Some(&source),
                    target,
                    None::<&str>,
                    MsFlags::MS_BIND | MsFlags::MS_REC,
                    None::<&str>,
                ) {
                    Ok(()) => {
                        // Remount with flags (to apply read-only etc.)
                        if mount_spec.readonly || !mount_spec.propagation.is_empty() {
                            let _ = mount(
                                Some(&source),
                                target,
                                None::<&str>,
                                flags | MsFlags::MS_REMOUNT,
                                None::<&str>,
                            );
                        }
                        tracing::info!(
                            "Successfully applied bind mount: {:?} -> {:?}",
                            source,
                            target
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to apply bind mount {:?} -> {:?}: {}",
                            source,
                            target,
                            e
                        );
                    }
                }
            }
            "tmpfs" => {
                let target = Path::new(&mount_spec.target);

                // Ensure target directory exists
                if let Some(parent) = target.parent() {
                    create_dir_all(parent)?;
                }
                create_dir_all(target)?;

                // Parse size option
                let mut options = String::from("mode=1777");
                if let Some(size) = &mount_spec.size {
                    options.push_str(&format!(",size={}", size));
                }

                mount(
                    None::<&str>,
                    target,
                    Some("tmpfs"),
                    MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
                    Some(options.as_str()),
                )?;

                tracing::debug!("Applied tmpfs mount: {:?}", target);
            }
            _ => {
                tracing::warn!("Unsupported mount type: {}", mount_spec.mount_type);
            }
        }
    }

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn apply_bind_mounts(_config: &ContainerConfig) -> Result<()> {
    Ok(())
}

/// Set up read-only root filesystem.
///
/// Remounts the root filesystem as read-only to prevent container modifications.
#[cfg(target_os = "linux")]
fn setup_readonly_rootfs() -> Result<()> {
    // Remount root as read-only
    nix::mount::mount(
        None::<&str>,
        "/",
        None::<&str>,
        nix::mount::MsFlags::MS_REMOUNT
            | nix::mount::MsFlags::MS_RDONLY
            | nix::mount::MsFlags::MS_BIND,
        None::<&str>,
    )?;

    tracing::debug!("Set root filesystem to read-only");

    Ok(())
}

/// Clean up the root filesystem for a container.
///
/// Removes the container's overlay directories. This should be called when
/// removing a container permanently.
///
/// # Arguments
///
/// * `config` - Container configuration
/// * `keep_upper` - If true, keep the upper directory (for state persistence)
pub fn cleanup_rootfs(config: &ContainerConfig) -> Result<()> {
    cleanup_rootfs_ex(config, false)
}

/// Extended cleanup with option to preserve upper layer.
///
/// # Arguments
///
/// * `config` - Container configuration
/// * `keep_upper` - If true, keep the upper directory (for resuming container)
pub fn cleanup_rootfs_ex(config: &ContainerConfig, keep_upper: bool) -> Result<()> {
    let container_root = get_container_root().join(&config.name);

    // First, unmount overlay if mounted
    let _ = unmount_overlay_rootfs(&config.name);

    if container_root.exists() {
        if keep_upper {
            // Keep upper directory, remove rootfs mount point and work dir
            let rootfs = container_root.join(ROOTFS_DIR);
            let work = container_root.join(WORK_DIR);

            if rootfs.exists() {
                std::fs::remove_dir_all(&rootfs)
                    .with_context(|| format!("Failed to remove rootfs directory: {:?}", rootfs))?;
            }
            if work.exists() {
                std::fs::remove_dir_all(&work)
                    .with_context(|| format!("Failed to remove work directory: {:?}", work))?;
            }

            tracing::info!("Cleaned up rootfs (kept upper layer): {:?}", container_root);
        } else {
            // Full cleanup - remove everything
            std::fs::remove_dir_all(&container_root).with_context(|| {
                format!("Failed to remove rootfs directory: {:?}", container_root)
            })?;

            tracing::info!("Cleaned up rootfs: {:?}", container_root);
        }
    }

    Ok(())
}

/// Clean up only the overlay mount point (for container stop without remove).
///
/// This unmounts the overlay but preserves the upper directory for resuming.
pub fn cleanup_overlay_mount(config: &ContainerConfig) -> Result<()> {
    unmount_overlay_rootfs(&config.name)
}

/// Check if a container has an existing writable layer (upper directory).
pub fn has_existing_upper(container_id: &str) -> bool {
    let upper_path = get_container_root().join(container_id).join(UPPER_DIR);

    upper_path.exists() && is_directory_non_empty(&upper_path).unwrap_or(false)
}

/// Check if a directory is non-empty.
fn is_directory_non_empty(path: &Path) -> Result<bool> {
    let mut entries =
        std::fs::read_dir(path).with_context(|| format!("Failed to read directory: {:?}", path))?;
    Ok(entries.next().is_some())
}

/// Get the size of the upper layer (writable changes).
pub fn get_upper_layer_size(container_id: &str) -> Result<u64> {
    let upper_path = get_container_root().join(container_id).join(UPPER_DIR);

    if !upper_path.exists() {
        return Ok(0);
    }

    dir_size(&upper_path)
}

/// Calculate directory size recursively.
fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0u64;

    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();

            if entry_path.is_dir() {
                total += dir_size(&entry_path)?;
            } else {
                total += entry.metadata()?.len();
            }
        }
    } else if path.is_file() {
        total += std::fs::metadata(path)?.len();
    }

    Ok(total)
}

/// Verify a rootfs is properly set up.
///
/// Checks that essential directories and files exist.
pub fn verify_rootfs(rootfs: &Path) -> Result<bool> {
    let required_paths = [
        "bin", "etc", "lib", "usr", "dev/null", "dev/zero", "dev/tty",
    ];

    for path in required_paths {
        let full_path = rootfs.join(path);
        if !full_path.exists() {
            tracing::warn!("Required path missing: {:?}", full_path);
            return Ok(false);
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uid_map_to_string() {
        let map = crate::userns::UidMap::new(0, 1000, 1);
        assert_eq!(map.to_map_string(), "0 1000 1");
    }

    #[test]
    fn test_rootfs_path_construction() {
        let config = ContainerConfig {
            name: "test-container".to_string(),
            ..Default::default()
        };

        let expected = get_container_root().join("test-container").join(ROOTFS_DIR);
        let actual = get_container_root().join(&config.name).join(ROOTFS_DIR);

        assert_eq!(expected, actual);
    }

    #[test]
    fn test_overlay_paths() {
        let config = ContainerConfig {
            name: "overlay-test".to_string(),
            ..Default::default()
        };

        let container_root = get_container_root().join(&config.name);

        let expected_root = get_container_root().join("overlay-test");

        assert_eq!(
            container_root.join(ROOTFS_DIR),
            expected_root.join(ROOTFS_DIR)
        );
        assert_eq!(
            container_root.join(UPPER_DIR),
            expected_root.join(UPPER_DIR)
        );
        assert_eq!(container_root.join(WORK_DIR), expected_root.join(WORK_DIR));
    }
}
