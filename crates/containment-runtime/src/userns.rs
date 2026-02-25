//! User namespace management for rootless containers.
//!
//! User namespaces allow containers to run without root privileges on the host
//! by mapping container UIDs/GIDs to host UIDs/GIDs. This is critical for
//! rootless container operation.
//!
//! # Example
//!
//! ```no_run
//! use containment_runtime::userns::{UidMap, GidMap, setup_user_namespace};
//! use nix::unistd::Pid;
//!
//! // Map container root (0) to host user (1000)
//! let uid_map = UidMap {
//!     inside_uid: 0,
//!     outside_uid: 1000,
//!     count: 1,
//! };
//! let gid_map = GidMap {
//!     inside_gid: 0,
//!     outside_gid: 1000,
//!     count: 1,
//! };
//!
//! setup_user_namespace(Pid::from_raw(1234), &uid_map, &gid_map)?;
//! ```

use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[cfg(target_os = "linux")]
use nix::unistd::Pid;

/// UID mapping configuration for user namespaces.
///
/// Defines how UIDs inside the container map to UIDs on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UidMap {
    /// First UID inside the namespace
    pub inside_uid: u32,

    /// First UID outside the namespace (on host)
    pub outside_uid: u32,

    /// Number of UIDs to map
    pub count: u32,
}

impl UidMap {
    /// Create a new UID mapping.
    pub const fn new(inside_uid: u32, outside_uid: u32, count: u32) -> Self {
        Self {
            inside_uid,
            outside_uid,
            count,
        }
    }

    /// Create a single UID mapping (e.g., root to current user).
    pub fn single(container_uid: u32, host_uid: u32) -> Self {
        Self::new(container_uid, host_uid, 1)
    }

    /// Get the current user's UID as a single mapping.
    #[cfg(target_os = "linux")]
    pub fn current_user() -> Self {
        Self::single(0, nix::unistd::Uid::current().as_raw())
    }

    /// Create a standard rootless container mapping.
    #[cfg(target_os = "linux")]
    pub fn rootless() -> Result<Self> {
        let host_uid = nix::unistd::Uid::current().as_raw();
        Ok(Self::new(0, host_uid, 1))
    }

    /// Convert to the format expected by /proc/[pid]/uid_map.
    pub fn to_map_string(&self) -> String {
        format!("{} {} {}", self.inside_uid, self.outside_uid, self.count)
    }
}

/// GID mapping configuration for user namespaces.
///
/// Defines how GIDs inside the container map to GIDs on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GidMap {
    /// First GID inside the namespace
    pub inside_gid: u32,

    /// First GID outside the namespace (on host)
    pub outside_gid: u32,

    /// Number of GIDs to map
    pub count: u32,
}

impl GidMap {
    /// Create a new GID mapping.
    pub const fn new(inside_gid: u32, outside_gid: u32, count: u32) -> Self {
        Self {
            inside_gid,
            outside_gid,
            count,
        }
    }

    /// Create a single GID mapping (e.g., root to current group).
    pub fn single(container_gid: u32, host_gid: u32) -> Self {
        Self::new(container_gid, host_gid, 1)
    }

    /// Get the current user's primary GID as a single mapping.
    #[cfg(target_os = "linux")]
    pub fn current_group() -> Self {
        Self::single(0, nix::unistd::Gid::current().as_raw())
    }

    /// Create a standard rootless container mapping.
    #[cfg(target_os = "linux")]
    pub fn rootless() -> Result<Self> {
        let host_gid = nix::unistd::Gid::current().as_raw();
        Ok(Self::new(0, host_gid, 1))
    }

    /// Convert to the format expected by /proc/[pid]/gid_map.
    pub fn to_map_string(&self) -> String {
        format!("{} {} {}", self.inside_gid, self.outside_gid, self.count)
    }
}

/// Write UID map to /proc/[pid]/uid_map for a process.
///
/// This must be done after creating a user namespace. The caller typically
/// needs CAP_SETUID in the parent namespace (or be the parent of the process).
///
/// # Arguments
///
/// * `pid` - The process ID whose user namespace we're configuring
/// * `uid_map` - The UID mapping to write
#[cfg(target_os = "linux")]
pub fn write_uid_map(pid: Pid, uid_map: &UidMap) -> Result<()> {
    let path = format!("/proc/{}/uid_map", pid.as_raw());

    // The file can only be written once, and only by a process with
    // the appropriate permissions in the parent namespace
    let mut file = OpenOptions::new()
        .write(true)
        .open(&path)
        .with_context(|| format!("Failed to open {}", path))?;

    writeln!(file, "{}", uid_map.to_map_string())
        .with_context(|| format!("Failed to write UID map to {}", path))?;

    tracing::debug!("Wrote UID map for PID {}: {}", pid.as_raw(), uid_map.to_map_string());

    Ok(())
}

/// Write GID map to /proc/[pid]/gid_map for a process.
///
/// This must be done after creating a user namespace. Similar to write_uid_map,
/// the caller needs appropriate permissions.
///
/// # Arguments
///
/// * `pid` - The process ID whose user namespace we're configuring
/// * `gid_map` - The GID mapping to write
#[cfg(target_os = "linux")]
pub fn write_gid_map(pid: Pid, gid_map: &GidMap) -> Result<()> {
    let path = format!("/proc/{}/gid_map", pid.as_raw());

    // GID map writing also requires disabling setgroups for child processes
    let setgroups_path = format!("/proc/{}/setgroups", pid.as_raw());
    if Path::new(&setgroups_path).exists() {
        std::fs::write(&setgroups_path, b"deny")
            .with_context(|| format!("Failed to write 'deny' to {}", setgroups_path))?;
    }

    let mut file = OpenOptions::new()
        .write(true)
        .open(&path)
        .with_context(|| format!("Failed to open {}", path))?;

    writeln!(file, "{}", gid_map.to_map_string())
        .with_context(|| format!("Failed to write GID map to {}", path))?;

    tracing::debug!("Wrote GID map for PID {}: {}", pid.as_raw(), gid_map.to_map_string());

    Ok(())
}

/// Set up user namespace for a container process.
///
/// This function writes both UID and GID maps for a process that has already
/// created a new user namespace via unshare(2) or clone(2).
///
/// # Important Ordering
///
/// User namespaces must be created FIRST before other namespaces because:
/// 1. The unshare() syscall for user namespace has specific requirements
/// 2. Other namespaces may require privileges that are only available after user namespace setup
/// 3. UID/GID mappings must be established before any privilege-dependent operations
///
/// # Arguments
///
/// * `pid` - The process ID (typically the child after fork)
/// * `uid_map` - UID mapping configuration
/// * `gid_map` - GID mapping configuration
#[cfg(target_os = "linux")]
pub fn setup_user_namespace(pid: Pid, uid_map: &UidMap, gid_map: &GidMap) -> Result<()> {
    // Write GID map first (required by some kernels)
    write_gid_map(pid, gid_map)?;

    // Then write UID map
    write_uid_map(pid, uid_map)?;

    tracing::info!("User namespace configured for PID {}", pid.as_raw());

    Ok(())
}

/// Non-Linux stub implementations
#[cfg(not(target_os = "linux"))]
pub fn setup_user_namespace(
    _pid: crate::namespace::ProcessId,
    _uid_map: &UidMap,
    _gid_map: &GidMap,
) -> Result<()> {
    Err(anyhow::anyhow!("User namespaces are only supported on Linux"))
}

/// Get the UID/GID maps for the current process.
///
/// Returns the current UID and GID mappings from /proc/self/uid_map and
/// /proc/self/gid_map. Useful for introspection.
#[cfg(target_os = "linux")]
pub fn get_current_maps() -> Result<(Vec<UidMap>, Vec<GidMap>)> {
    let uid_map_content = std::fs::read_to_string("/proc/self/uid_map")?;
    let gid_map_content = std::fs::read_to_string("/proc/self/gid_map")?;

    let uid_maps = parse_uid_maps(&uid_map_content)?;
    let gid_maps = parse_gid_maps(&gid_map_content)?;

    Ok((uid_maps, gid_maps))
}

/// Parse UID map content from /proc/[pid]/uid_map.
#[cfg(target_os = "linux")]
fn parse_uid_maps(content: &str) -> Result<Vec<UidMap>> {
    let mut maps = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 3 {
            continue;
        }

        let inside: u32 = parts[0].parse()
            .context("Invalid UID map inside UID")?;
        let outside: u32 = parts[1].parse()
            .context("Invalid UID map outside UID")?;
        let count: u32 = parts[2].parse()
            .context("Invalid UID map count")?;

        maps.push(UidMap::new(inside, outside, count));
    }

    Ok(maps)
}

/// Parse GID map content from /proc/[pid]/gid_map.
#[cfg(target_os = "linux")]
fn parse_gid_maps(content: &str) -> Result<Vec<GidMap>> {
    let mut maps = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 3 {
            continue;
        }

        let inside: u32 = parts[0].parse()
            .context("Invalid GID map inside GID")?;
        let outside: u32 = parts[1].parse()
            .context("Invalid GID map outside GID")?;
        let count: u32 = parts[2].parse()
            .context("Invalid GID map count")?;

        maps.push(GidMap::new(inside, outside, count));
    }

    Ok(maps)
}

/// Check if we're running in a user namespace.
#[cfg(target_os = "linux")]
pub fn in_user_namespace() -> bool {
    // Compare our UID maps with the init process
    // If they differ, we're in a user namespace
    if let Ok((uid_maps, _)) = get_current_maps() {
        // A simple check: if we have any UID map entries, we might be in a user namespace
        // The definitive check is comparing /proc/self/uid_map with /proc/1/uid_map
        if let Ok(init_uid_map) = std::fs::read_to_string("/proc/1/uid_map") {
            let our_map = std::fs::read_to_string("/proc/self/uid_map").unwrap_or_default();
            return uid_maps.len() > 0 && our_map != init_uid_map;
        }
    }
    false
}

/// Non-Linux stub implementations
#[cfg(not(target_os = "linux"))]
pub fn in_user_namespace() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uid_map_to_string() {
        let map = UidMap::new(0, 1000, 1);
        assert_eq!(map.to_map_string(), "0 1000 1");

        let map = UidMap::new(0, 100000, 65536);
        assert_eq!(map.to_map_string(), "0 100000 65536");
    }

    #[test]
    fn test_gid_map_to_string() {
        let map = GidMap::new(0, 1000, 1);
        assert_eq!(map.to_map_string(), "0 1000 1");
    }

    #[test]
    fn test_single_mapping() {
        let uid_map = UidMap::single(0, 1000);
        assert_eq!(uid_map.inside_uid, 0);
        assert_eq!(uid_map.outside_uid, 1000);
        assert_eq!(uid_map.count, 1);

        let gid_map = GidMap::single(0, 1000);
        assert_eq!(gid_map.inside_gid, 0);
        assert_eq!(gid_map.outside_gid, 1000);
        assert_eq!(gid_map.count, 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_rootless_mapping() {
        let uid_map = UidMap::rootless().unwrap();
        assert_eq!(uid_map.inside_uid, 0);
        // outside_uid should be current user's UID
        assert!(uid_map.outside_uid > 0);

        let gid_map = GidMap::rootless().unwrap();
        assert_eq!(gid_map.inside_gid, 0);
        assert!(gid_map.outside_gid > 0);
    }

    #[test]
    fn test_parse_uid_maps() {
        #[cfg(target_os = "linux")]
        {
            let content = "         0          0          1\n";
            let maps = parse_uid_maps(content).unwrap();
            assert_eq!(maps.len(), 1);
            assert_eq!(maps[0].inside_uid, 0);
            assert_eq!(maps[0].outside_uid, 0);
            assert_eq!(maps[0].count, 1);
        }
    }
}
