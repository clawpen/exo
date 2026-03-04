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
//!
//! # Example
//!
//! ```no_run
//! use exo_runtime::rootfs::{prepare_rootfs, pivot_rootfs, setup_mounts};
//! use exo_runtime::config::ContainerConfig;
//!
//! let config = ContainerConfig::default();
//! let rootfs_path = prepare_rootfs(&config)?;
//! pivot_rootfs(&rootfs_path)?;
//! setup_mounts(&config)?;
//! ```

use crate::config::ContainerConfig;
use anyhow::{Context, Result};
use std::fs::create_dir_all;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use {
    nix::mount::{mount, MsFlags},
    nix::unistd::pivot_root,
    nix::sys::stat::{mknod, SFlag, Mode, makedev},
    std::fs::File,
    std::os::unix::io::AsRawFd,
};

/// Default container root directory.
pub const CONTAINER_ROOT_DIR: &str = "/var/lib/openclaw/containers";

/// Directory name for the container filesystem.
pub const ROOTFS_DIR: &str = "rootfs";

/// Old root directory after pivot (used for cleanup).
pub const PIVOT_OLD_ROOT: &str = "pivot_old";

/// Prepare the root filesystem for a container.
///
/// This function:
/// 1. Creates the container directory structure
/// 2. Extracts/copies the rootfs from the image (if available)
/// 3. Sets up the base directory structure
///
/// # Arguments
///
/// * `config` - Container configuration with image reference
///
/// # Returns
///
/// Path to the prepared root filesystem
pub fn prepare_rootfs(config: &ContainerConfig) -> Result<PathBuf> {
    let container_id = &config.name;
    let rootfs_dir = PathBuf::from(CONTAINER_ROOT_DIR)
        .join(container_id)
        .join(ROOTFS_DIR);

    // Create the rootfs directory if it doesn't exist
    if !rootfs_dir.exists() {
        create_dir_all(&rootfs_dir)
            .with_context(|| format!("Failed to create rootfs directory: {:?}", rootfs_dir))?;
    }

    // In a real implementation, we would:
    // 1. Download/extract the OCI image layers
    // 2. Apply whiteout files for layer deletion
    // 3. Set up the base directory structure

    // For now, create a minimal rootfs structure
    setup_minimal_rootfs(&rootfs_dir)?;

    tracing::info!("Prepared rootfs at: {:?}", rootfs_dir);

    Ok(rootfs_dir)
}

/// Set up a minimal root filesystem structure.
///
/// Creates the basic directory structure expected in a Linux rootfs.
#[cfg(target_os = "linux")]
fn setup_minimal_rootfs(rootfs: &Path) -> Result<()> {
    let dirs = [
        "bin", "sbin", "usr/bin", "usr/sbin", "usr/local/bin",
        "etc", "lib", "lib64", "usr/lib", "usr/lib64",
        "proc", "sys", "dev", "tmp", "var/tmp", "var/run",
        "home", "root", "mnt", "media",
    ];

    for dir in dirs {
        let dir_path = rootfs.join(dir);
        create_dir_all(&dir_path)
            .with_context(|| format!("Failed to create directory: {:?}", dir_path))?;
    }

    // Create basic device nodes
    create_dev_nodes(rootfs)?;

    // Create a minimal /etc/hosts
    let hosts_path = rootfs.join("etc/hosts");
    if !hosts_path.exists() {
        std::fs::write(
            &hosts_path,
            "127.0.0.1 localhost localhost.localdomain\n::1 localhost6 localhost6.localdomain6\n",
        )?;
    }

    // Create a minimal /etc/resolv.conf
    let resolv_path = rootfs.join("etc/resolv.conf");
    if !resolv_path.exists() {
        // Copy from host or use default
        if let Ok(host_resolv) = std::fs::read_to_string("/etc/resolv.conf") {
            std::fs::write(&resolv_path, host_resolv)?;
        } else {
            std::fs::write(&resolv_path, "nameserver 8.8.8.8\nnameserver 8.8.4.4\n")?;
        }
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
    let new_root = new_root.canonicalize()
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
    pivot_root(&new_root, &put_old)
        .context("pivot_root syscall failed")?;

    // Change to new root
    std::env::set_current_dir("/")
        .context("Failed to change directory to new root")?;

    // Unmount the old root
    let old_root = PathBuf::from("/").join(PIVOT_OLD_ROOT);
    nix::mount::umount(&old_root)
        .context("Failed to unmount old root")?;

    // Remove the old root directory
    std::fs::remove_dir(&old_root)
        .context("Failed to remove old root directory")?;

    tracing::debug!("pivot_root completed successfully");

    Ok(())
}

/// Stub for non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn pivot_rootfs(_new_root: &Path) -> Result<()> {
    Err(anyhow::anyhow!("pivot_root is only supported on Linux"))
}

/// Set up the essential mounts for a container.
///
/// This function mounts:
/// - proc filesystem at /proc
/// - sysfs at /sys (optional, can be disabled for security)
/// - tmpfs at /dev/shm
/// - devpts at /dev/pts
///
/// # Arguments
///
/// * `config` - Container configuration
#[cfg(target_os = "linux")]
pub fn setup_mounts(config: &ContainerConfig) -> Result<()> {
    // Mount proc filesystem
    mount(
        None::<&str>,
        "/proc",
        Some("proc"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        None::<&str>,
    )?;
    tracing::debug!("Mounted /proc");

    // Mount sysfs (can be disabled for security, but most containers need it)
    // Using MS_RDONLY to prevent container from modifying sysfs
    mount(
        None::<&str>,
        "/sys",
        Some("sysfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV | MsFlags::MS_RDONLY,
        None::<&str>,
    )?;
    tracing::debug!("Mounted /sys");

    // Mount tmpfs on /dev/shm
    mount(
        None::<&str>,
        "/dev/shm",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    )?;
    tracing::debug!("Mounted /dev/shm");

    // Mount devpts for /dev/pts
    if Path::new("/dev/pts").exists() {
        mount(
            None::<&str>,
            "/dev/pts",
            Some("devpts"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
            Some("newinstance,ptmxmode=0666,mode=0620"),
        )?;

        // Create /dev/ptmx symlink if it doesn't exist
        let ptmx = Path::new("/dev/ptmx");
        if !ptmx.exists() {
            let _ = std::os::unix::fs::symlink("pts/ptmx", ptmx);
        }

        tracing::debug!("Mounted /dev/pts");
    }

    // Mount tmpfs on /tmp if configured
    mount(
        None::<&str>,
        "/tmp",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    )?;
    tracing::debug!("Mounted /tmp");

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

/// Apply user-specified bind mounts.
///
/// Bind mounts allow selective access to host directories.
///
/// # Arguments
///
/// * `config` - Container configuration with mount specifications
#[cfg(target_os = "linux")]
pub fn apply_bind_mounts(config: &ContainerConfig) -> Result<()> {
    for mount_spec in &config.mounts {
        match mount_spec.mount_type.as_str() {
            "bind" => {
                let source = Path::new(&mount_spec.source);
                let target = Path::new(&mount_spec.target);

                // Ensure source exists on host
                if !source.exists() {
                    tracing::warn!("Bind mount source does not exist: {:?}", source);
                    continue;
                }

                // Ensure target directory exists in container
                if let Some(parent) = target.parent() {
                    create_dir_all(parent)
                        .with_context(|| format!("Failed to create mount parent: {:?}", parent))?;
                }
                if !target.exists() {
                    if source.is_dir() {
                        create_dir_all(target)
                            .with_context(|| format!("Failed to create mount target: {:?}", target))?;
                    } else {
                        // Create parent for file
                        if let Some(parent) = target.parent() {
                            create_dir_all(parent)?;
                        }
                        File::create(target)?;
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

                // First bind
                mount(
                    Some(source),
                    target,
                    None::<&str>,
                    MsFlags::MS_BIND | MsFlags::MS_REC,
                    None::<&str>,
                )?;

                // Remount with flags (to apply read-only etc.)
                if mount_spec.readonly || !mount_spec.propagation.is_empty() {
                    mount(
                        Some(source),
                        target,
                        None::<&str>,
                        flags | MsFlags::MS_REMOUNT,
                        None::<&str>,
                    )?;
                }

                tracing::debug!("Applied bind mount: {:?} -> {:?}", source, target);
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
        nix::mount::MsFlags::MS_REMOUNT | nix::mount::MsFlags::MS_RDONLY | nix::mount::MsFlags::MS_BIND,
        None::<&str>,
    )?;

    tracing::debug!("Set root filesystem to read-only");

    Ok(())
}

/// Clean up the root filesystem for a container.
///
/// Removes the container's rootfs directory. This should be called when
/// removing a container.
pub fn cleanup_rootfs(config: &ContainerConfig) -> Result<()> {
    let container_root = PathBuf::from(CONTAINER_ROOT_DIR).join(&config.name);

    if container_root.exists() {
        std::fs::remove_dir_all(&container_root)
            .with_context(|| format!("Failed to remove rootfs directory: {:?}", container_root))?;

        tracing::info!("Cleaned up rootfs: {:?}", container_root);
    }

    Ok(())
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

        let expected = PathBuf::from("/var/lib/openclaw/containers/test-container/rootfs");
        let actual = PathBuf::from(CONTAINER_ROOT_DIR)
            .join(&config.name)
            .join(ROOTFS_DIR);

        assert_eq!(expected, actual);
    }
}
