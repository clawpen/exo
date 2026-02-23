//! Mount operations for container filesystems.

use crate::container::MountSpec;
use anyhow::Result;

#[cfg(target_os = "linux")]
use nix::mount::{MsFlags, mount, umount};
#[cfg(target_os = "linux")]
use std::path::Path;

/// Mount setup for containers.
pub struct MountSetup {
    rootfs: String,
}

impl MountSetup {
    /// Create a new mount setup for the given rootfs.
    pub fn new(rootfs: &str) -> Self {
        Self {
            rootfs: rootfs.to_string(),
        }
    }

    /// Set up all mounts for the container.
    pub fn setup_mounts(&self, _mounts: &[MountSpec]) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            // Create essential mount points
            self.setup_essential_mounts()?;

            // Set up user-specified mounts
            for mount_spec in mounts {
                self.setup_mount(mount_spec)?;
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("Mount setup only supported on Linux");
        }

        Ok(())
    }

    /// Set up essential filesystem mounts (proc, sys, dev, etc.).
    #[cfg(target_os = "linux")]
    fn setup_essential_mounts(&self) -> Result<()> {
        use std::path::Path;

        let root = Path::new(&self.rootfs);

        // Create necessary directories
        self.mkdir_p(root.join("proc"))?;
        self.mkdir_p(root.join("sys"))?;
        self.mkdir_p(root.join("dev"))?;
        self.mkdir_p(root.join("dev/shm"))?;
        self.mkdir_p(root.join("dev/pts"))?;
        self.mkdir_p(root.join("tmp"))?;
        self.mkdir_p(root.join("run"))?;

        // Mount proc
        mount(
            Some("proc"),
            &root.join("proc"),
            Some("proc"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
            None::<&str>,
        )?;
        tracing::debug!("Mounted proc");

        // Mount sys
        mount(
            Some("sysfs"),
            &root.join("sys"),
            Some("sysfs"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV | MsFlags::MS_RDONLY,
            None::<&str>,
        )?;
        tracing::debug!("Mounted sysfs");

        // Mount dev (as tmpfs for security)
        mount(
            Some("tmpfs"),
            &root.join("dev"),
            Some("tmpfs"),
            MsFlags::MS_NOSUID | MsFlags::MS_STRICTATIME,
            Some("mode=755"),
        )?;
        tracing::debug!("Mounted dev tmpfs");

        // Create device nodes
        self.create_device_nodes(root)?;

        // Mount dev/pts
        self.mkdir_p(root.join("dev/pts"))?;
        mount(
            Some("devpts"),
            &root.join("dev/pts"),
            Some("devpts"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
            None::<&str>,
        )?;

        // Mount dev/shm
        mount(
            Some("tmpfs"),
            &root.join("dev/shm"),
            Some("tmpfs"),
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None::<&str>,
        )?;

        // Mount tmpfs on /tmp
        mount(
            Some("tmpfs"),
            &root.join("tmp"),
            Some("tmpfs"),
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None::<&str>,
        )?;

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn setup_essential_mounts(&self) -> Result<()> {
        Ok(())
    }

    /// Create essential device nodes.
    #[cfg(target_os = "linux")]
    fn create_device_nodes(&self, root: &Path) -> Result<()> {
        use nix::sys::stat::{mknod, Mode, SFlag};
        use std::os::unix::fs::PermissionsExt;

        // /dev/null
        mknod(
            &root.join("dev/null"),
            SFlag::S_IFCHR,
            Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IWGRP | Mode::S_IROTH | Mode::S_IWOTH,
            libc::dev_t::from(1u32 << 8 | 3u32),
        )?;

        // /dev/zero
        mknod(
            &root.join("dev/zero"),
            SFlag::S_IFCHR,
            Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IWGRP | Mode::S_IROTH | Mode::S_IWOTH,
            libc::dev_t::from(1u32 << 8 | 5u32),
        )?;

        // /dev/full
        mknod(
            &root.join("dev/full"),
            SFlag::S_IFCHR,
            Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IWGRP | Mode::S_IROTH | Mode::S_IWOTH,
            libc::dev_t::from(1u32 << 8 | 7u32),
        )?;

        // /dev/random
        mknod(
            &root.join("dev/random"),
            SFlag::S_IFCHR,
            Mode::S_IRUSR | Mode::S_IRGRP | Mode::S_IROTH,
            libc::dev_t::from(1u32 << 8 | 8u32),
        )?;

        // /dev/urandom
        mknod(
            &root.join("dev/urandom"),
            SFlag::S_IFCHR,
            Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IWGRP | Mode::S_IROTH | Mode::S_IWOTH,
            libc::dev_t::from(1u32 << 8 | 9u32),
        )?;

        // /dev/tty
        mknod(
            &root.join("dev/tty"),
            SFlag::S_IFCHR,
            Mode::S_IRUSR | Mode::S_IWUSR | Mode::S_IRGRP | Mode::S_IWGRP | Mode::S_IROTH | Mode::S_IWOTH,
            libc::dev_t::from(5u32 << 8 | 0u32),
        )?;

        // Create symlinks
        std::os::unix::fs::symlink("/proc/self/fd", root.join("dev/fd"))?;
        std::os::unix::fs::symlink("/proc/self/fd/0", root.join("dev/stdin"))?;
        std::os::unix::fs::symlink("/proc/self/fd/1", root.join("dev/stdout"))?;
        std::os::unix::fs::symlink("/proc/self/fd/2", root.join("dev/stderr"))?;

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn create_device_nodes(&self, _root: &std::path::Path) -> Result<()> {
        Ok(())
    }

    /// Set up a user-specified mount.
    #[cfg(target_os = "linux")]
    fn setup_mount(&self, mount_spec: &MountSpec) -> Result<()> {
        use std::path::Path;

        let target = Path::new(&self.rootfs).join(&mount_spec.target.strip_prefix('/').unwrap_or(&mount_spec.target));

        // Create target directory if needed
        self.mkdir_p(&target)?;

        let mut flags = MsFlags::MS_BIND | MsFlags::MS_NOSUID | MsFlags::MS_NODEV;
        if mount_spec.readonly {
            flags |= MsFlags::MS_RDONLY;
        }

        mount(
            Some(&mount_spec.source),
            &target,
            None::<&str>,
            flags,
            None::<&str>,
        )?;

        tracing::debug!("Mounted {} to {}", mount_spec.source, mount_spec.target);

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    fn setup_mount(&self, _mount_spec: &MountSpec) -> Result<()> {
        Ok(())
    }

    /// Pivot to the new root filesystem.
    #[cfg(target_os = "linux")]
    pub fn pivot_root(&self) -> Result<()> {
        use nix::unistd::pivot_root;
        use std::path::Path;

        let new_root = Path::new(&self.rootfs);
        let put_old = new_root.join(".pivot_root");

        // Create the old root directory
        std::fs::create_dir_all(&put_old)?;

        // Pivot root
        pivot_root(new_root, &put_old)?;

        // Unmount the old root
        umount(&put_old, MsFlags::MS_DETACH)?;

        // Remove the old root directory
        std::fs::remove_dir_all(&put_old)?;

        tracing::debug!("Pivoted to new root: {}", self.rootfs);

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn pivot_root(&self) -> Result<()> {
        Err(anyhow::anyhow!("pivot_root only supported on Linux"))
    }

    /// Change to the new root (simple alternative to pivot_root).
    #[cfg(target_os = "linux")]
    pub fn chroot_root(&self) -> Result<()> {
        use nix::unistd::{chroot, chdir};
        use std::path::Path;

        let root = Path::new(&self.rootfs);

        // Change to new root
        chdir(root)?;
        chroot(root)?;

        // Change to /
        chdir("/")?;

        tracing::debug!("Chrooted to: {}", self.rootfs);

        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn chroot_root(&self) -> Result<()> {
        tracing::warn!("chroot only supported on Linux");
        Ok(())
    }

    fn mkdir_p(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        std::fs::create_dir_all(path.as_ref())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mount_setup_new() {
        let setup = MountSetup::new("/tmp/test_rootfs");
        assert_eq!(setup.rootfs, "/tmp/test_rootfs");
    }
}
