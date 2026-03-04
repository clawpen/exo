//! Linux capabilities and security management for containers.
//!
//! This module handles dropping unnecessary Linux capabilities to reduce
//! the attack surface of containers. Capabilities are fine-grained privileges
//! that can be independently enabled/disabled for processes.
//!
//! # Why Drop Capabilities?
//!
//! By default, containers run with many capabilities that they don't need.
//! Dropping unnecessary capabilities limits what a compromised container
//! process can do on the host system.
//!
//! # Example
//!
//! ```no_run
//! use exo_runtime::security::{Capability, drop_capabilities};
//!
//! // Keep only essential capabilities for a web container
//! let keep = vec![
//!     Capability::CAP_NET_BIND_SERVICE,
//!     Capability::CAP_SETUID,
//!     Capability::CAP_SETGID,
//! ];
//! drop_capabilities(&keep)?;
//! ```

use anyhow::{Context, Result};
use std::collections::HashSet;

#[cfg(target_os = "linux")]
use {
    caps::{CapSet, Capability as CapsCapability},
    std::iter::FromIterator,
};

/// Linux capability flag.
///
/// Represents a specific Linux capability that can be granted or dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// CAP_AUDIT_CONTROL - Allow auditing subsystem configuration
    CAP_AUDIT_CONTROL,
    /// CAP_AUDIT_READ - Allow reading audit logs
    CAP_AUDIT_READ,
    /// CAP_AUDIT_WRITE - Allow writing to audit log
    CAP_AUDIT_WRITE,
    /// CAP_BLOCK_SUSPEND - Allow preventing system suspends
    CAP_BLOCK_SUSPEND,
    /// CAP_CHOWN - Allow changing file ownership
    CAP_CHOWN,
    /// CAP_DAC_OVERRIDE - Override file access permissions
    CAP_DAC_OVERRIDE,
    /// CAP_DAC_READ_SEARCH - Override file read/check permissions
    CAP_DAC_READ_SEARCH,
    /// CAP_FOWNER - Override file ownership restrictions
    CAP_FOWNER,
    /// CAP_FSETID - Override set-user/group ID mode setting
    CAP_FSETID,
    /// CAP_IPC_LOCK - Allow locking memory segments
    CAP_IPC_LOCK,
    /// CAP_IPC_OWNER - Override IPC ownership checks
    CAP_IPC_OWNER,
    /// CAP_KILL - Allow sending signals to other processes
    CAP_KILL,
    /// CAP_LEASE - Allow file leasing
    CAP_LEASE,
    /// CAP_LINUX_IMMUTABLE - Set immutable/append-only file attributes
    CAP_LINUX_IMMUTABLE,
    /// CAP_MAC_ADMIN - Allow MAC configuration
    CAP_MAC_ADMIN,
    /// CAP_MAC_OVERRIDE - Override MAC (Mandatory Access Control)
    CAP_MAC_OVERRIDE,
    /// CAP_MKNOD - Create special files via mknod
    CAP_MKNOD,
    /// CAP_NET_ADMIN - Network administration (firewall, routing, etc.)
    CAP_NET_ADMIN,
    /// CAP_NET_BIND_SERVICE - Bind to privileged ports (< 1024)
    CAP_NET_BIND_SERVICE,
    /// CAP_NET_BROADCAST - Allow socket broadcasting
    CAP_NET_BROADCAST,
    /// CAP_NET_RAW - Use raw sockets
    CAP_NET_RAW,
    /// CAP_SETGID - Set group ID
    CAP_SETGID,
    /// CAP_SETFCAP - Set file capabilities
    CAP_SETFCAP,
    /// CAP_SETPCAP - Modify process capabilities
    CAP_SETPCAP,
    /// CAP_SETUID - Set user ID
    CAP_SETUID,
    /// CAP_SYSLOG - Allow syslog operations
    CAP_SYSLOG,
    /// CAP_SYS_ADMIN - Full system administration
    CAP_SYS_ADMIN,
    /// CAP_SYS_BOOT - Allow rebooting
    CAP_SYS_BOOT,
    /// CAP_SYS_CHROOT - Use chroot()
    CAP_SYS_CHROOT,
    /// CAP_SYS_MODULE - Load/unload kernel modules
    CAP_SYS_MODULE,
    /// CAP_SYS_NICE - Change process priority
    CAP_SYS_NICE,
    /// CAP_SYS_PACCT - Allow process accounting
    CAP_SYS_PACCT,
    /// CAP_SYS_PTRACE - Use ptrace() on any process
    CAP_SYS_PTRACE,
    /// CAP_SYS_RAWIO - Raw I/O operations
    CAP_SYS_RAWIO,
    /// CAP_SYS_RESOURCE - Override resource limits
    CAP_SYS_RESOURCE,
    /// CAP_SYS_TIME - Set system clock
    CAP_SYS_TIME,
    /// CAP_SYS_TTY_CONFIG - Allow vhangup() on tty
    CAP_SYS_TTY_CONFIG,
    /// CAP_WAKE_ALARM - Allow setting system wake alarm
    CAP_WAKE_ALARM,
}

impl Capability {
    /// Get the name string for this capability.
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::CAP_AUDIT_CONTROL => "CAP_AUDIT_CONTROL",
            Capability::CAP_AUDIT_READ => "CAP_AUDIT_READ",
            Capability::CAP_AUDIT_WRITE => "CAP_AUDIT_WRITE",
            Capability::CAP_BLOCK_SUSPEND => "CAP_BLOCK_SUSPEND",
            Capability::CAP_CHOWN => "CAP_CHOWN",
            Capability::CAP_DAC_OVERRIDE => "CAP_DAC_OVERRIDE",
            Capability::CAP_DAC_READ_SEARCH => "CAP_DAC_READ_SEARCH",
            Capability::CAP_FOWNER => "CAP_FOWNER",
            Capability::CAP_FSETID => "CAP_FSETID",
            Capability::CAP_IPC_LOCK => "CAP_IPC_LOCK",
            Capability::CAP_IPC_OWNER => "CAP_IPC_OWNER",
            Capability::CAP_KILL => "CAP_KILL",
            Capability::CAP_LEASE => "CAP_LEASE",
            Capability::CAP_LINUX_IMMUTABLE => "CAP_LINUX_IMMUTABLE",
            Capability::CAP_MAC_ADMIN => "CAP_MAC_ADMIN",
            Capability::CAP_MAC_OVERRIDE => "CAP_MAC_OVERRIDE",
            Capability::CAP_MKNOD => "CAP_MKNOD",
            Capability::CAP_NET_ADMIN => "CAP_NET_ADMIN",
            Capability::CAP_NET_BIND_SERVICE => "CAP_NET_BIND_SERVICE",
            Capability::CAP_NET_BROADCAST => "CAP_NET_BROADCAST",
            Capability::CAP_NET_RAW => "CAP_NET_RAW",
            Capability::CAP_SETGID => "CAP_SETGID",
            Capability::CAP_SETFCAP => "CAP_SETFCAP",
            Capability::CAP_SETPCAP => "CAP_SETPCAP",
            Capability::CAP_SETUID => "CAP_SETUID",
            Capability::CAP_SYSLOG => "CAP_SYSLOG",
            Capability::CAP_SYS_ADMIN => "CAP_SYS_ADMIN",
            Capability::CAP_SYS_BOOT => "CAP_SYS_BOOT",
            Capability::CAP_SYS_CHROOT => "CAP_SYS_CHROOT",
            Capability::CAP_SYS_MODULE => "CAP_SYS_MODULE",
            Capability::CAP_SYS_NICE => "CAP_SYS_NICE",
            Capability::CAP_SYS_PACCT => "CAP_SYS_PACCT",
            Capability::CAP_SYS_PTRACE => "CAP_SYS_PTRACE",
            Capability::CAP_SYS_RAWIO => "CAP_SYS_RAWIO",
            Capability::CAP_SYS_RESOURCE => "CAP_SYS_RESOURCE",
            Capability::CAP_SYS_TIME => "CAP_SYS_TIME",
            Capability::CAP_SYS_TTY_CONFIG => "CAP_SYS_TTY_CONFIG",
            Capability::CAP_WAKE_ALARM => "CAP_WAKE_ALARM",
        }
    }

    /// Convert to caps crate Capability.
    #[cfg(target_os = "linux")]
    fn as_caps_capability(&self) -> CapsCapability {
        match self {
            Capability::CAP_AUDIT_CONTROL => CapsCapability::CAP_AUDIT_CONTROL,
            Capability::CAP_AUDIT_READ => CapsCapability::CAP_AUDIT_READ,
            Capability::CAP_AUDIT_WRITE => CapsCapability::CAP_AUDIT_WRITE,
            Capability::CAP_BLOCK_SUSPEND => CapsCapability::CAP_BLOCK_SUSPEND,
            Capability::CAP_CHOWN => CapsCapability::CAP_CHOWN,
            Capability::CAP_DAC_OVERRIDE => CapsCapability::CAP_DAC_OVERRIDE,
            Capability::CAP_DAC_READ_SEARCH => CapsCapability::CAP_DAC_READ_SEARCH,
            Capability::CAP_FOWNER => CapsCapability::CAP_FOWNER,
            Capability::CAP_FSETID => CapsCapability::CAP_FSETID,
            Capability::CAP_IPC_LOCK => CapsCapability::CAP_IPC_LOCK,
            Capability::CAP_IPC_OWNER => CapsCapability::CAP_IPC_OWNER,
            Capability::CAP_KILL => CapsCapability::CAP_KILL,
            Capability::CAP_LEASE => CapsCapability::CAP_LEASE,
            Capability::CAP_LINUX_IMMUTABLE => CapsCapability::CAP_LINUX_IMMUTABLE,
            Capability::CAP_MAC_ADMIN => CapsCapability::CAP_MAC_ADMIN,
            Capability::CAP_MAC_OVERRIDE => CapsCapability::CAP_MAC_OVERRIDE,
            Capability::CAP_MKNOD => CapsCapability::CAP_MKNOD,
            Capability::CAP_NET_ADMIN => CapsCapability::CAP_NET_ADMIN,
            Capability::CAP_NET_BIND_SERVICE => CapsCapability::CAP_NET_BIND_SERVICE,
            Capability::CAP_NET_BROADCAST => CapsCapability::CAP_NET_BROADCAST,
            Capability::CAP_NET_RAW => CapsCapability::CAP_NET_RAW,
            Capability::CAP_SETGID => CapsCapability::CAP_SETGID,
            Capability::CAP_SETFCAP => CapsCapability::CAP_SETFCAP,
            Capability::CAP_SETPCAP => CapsCapability::CAP_SETPCAP,
            Capability::CAP_SETUID => CapsCapability::CAP_SETUID,
            Capability::CAP_SYSLOG => CapsCapability::CAP_SYSLOG,
            Capability::CAP_SYS_ADMIN => CapsCapability::CAP_SYS_ADMIN,
            Capability::CAP_SYS_BOOT => CapsCapability::CAP_SYS_BOOT,
            Capability::CAP_CHOWN => CapsCapability::CAP_CHOWN,
            Capability::CAP_SYS_CHROOT => CapsCapability::CAP_SYS_CHROOT,
            Capability::CAP_SYS_MODULE => CapsCapability::CAP_SYS_MODULE,
            Capability::CAP_SYS_NICE => CapsCapability::CAP_SYS_NICE,
            Capability::CAP_SYS_PACCT => CapsCapability::CAP_SYS_PACCT,
            Capability::CAP_SYS_PTRACE => CapsCapability::CAP_SYS_PTRACE,
            Capability::CAP_SYS_RAWIO => CapsCapability::CAP_SYS_RAWIO,
            Capability::CAP_SYS_RESOURCE => CapsCapability::CAP_SYS_RESOURCE,
            Capability::CAP_SYS_TIME => CapsCapability::CAP_SYS_TIME,
            Capability::CAP_SYS_TTY_CONFIG => CapsCapability::CAP_SYS_TTY_CONFIG,
            Capability::CAP_WAKE_ALARM => CapsCapability::CAP_WAKE_ALARM,
        }
    }

    /// Get all defined capabilities.
    pub fn all() -> HashSet<Capability> {
        HashSet::from_iter(vec![
            Capability::CAP_AUDIT_CONTROL,
            Capability::CAP_AUDIT_READ,
            Capability::CAP_AUDIT_WRITE,
            Capability::CAP_BLOCK_SUSPEND,
            Capability::CAP_CHOWN,
            Capability::CAP_DAC_OVERRIDE,
            Capability::CAP_DAC_READ_SEARCH,
            Capability::CAP_FOWNER,
            Capability::CAP_FSETID,
            Capability::CAP_IPC_LOCK,
            Capability::CAP_IPC_OWNER,
            Capability::CAP_KILL,
            Capability::CAP_LEASE,
            Capability::CAP_LINUX_IMMUTABLE,
            Capability::CAP_MAC_ADMIN,
            Capability::CAP_MAC_OVERRIDE,
            Capability::CAP_MKNOD,
            Capability::CAP_NET_ADMIN,
            Capability::CAP_NET_BIND_SERVICE,
            Capability::CAP_NET_BROADCAST,
            Capability::CAP_NET_RAW,
            Capability::CAP_SETGID,
            Capability::CAP_SETFCAP,
            Capability::CAP_SETPCAP,
            Capability::CAP_SETUID,
            Capability::CAP_SYSLOG,
            Capability::CAP_SYS_ADMIN,
            Capability::CAP_SYS_BOOT,
            Capability::CAP_SYS_CHROOT,
            Capability::CAP_SYS_MODULE,
            Capability::CAP_SYS_NICE,
            Capability::CAP_SYS_PACCT,
            Capability::CAP_SYS_PTRACE,
            Capability::CAP_SYS_RAWIO,
            Capability::CAP_SYS_RESOURCE,
            Capability::CAP_SYS_TIME,
            Capability::CAP_SYS_TTY_CONFIG,
            Capability::CAP_WAKE_ALARM,
        ])
    }

    /// Parse capability from string name.
    pub fn from_str(s: &str) -> Option<Capability> {
        match s {
            "CAP_AUDIT_CONTROL" => Some(Capability::CAP_AUDIT_CONTROL),
            "CAP_AUDIT_READ" => Some(Capability::CAP_AUDIT_READ),
            "CAP_AUDIT_WRITE" => Some(Capability::CAP_AUDIT_WRITE),
            "CAP_BLOCK_SUSPEND" => Some(Capability::CAP_BLOCK_SUSPEND),
            "CAP_CHOWN" => Some(Capability::CAP_CHOWN),
            "CAP_DAC_OVERRIDE" => Some(Capability::CAP_DAC_OVERRIDE),
            "CAP_DAC_READ_SEARCH" => Some(Capability::CAP_DAC_READ_SEARCH),
            "CAP_FOWNER" => Some(Capability::CAP_FOWNER),
            "CAP_FSETID" => Some(Capability::CAP_FSETID),
            "CAP_IPC_LOCK" => Some(Capability::CAP_IPC_LOCK),
            "CAP_IPC_OWNER" => Some(Capability::CAP_IPC_OWNER),
            "CAP_KILL" => Some(Capability::CAP_KILL),
            "CAP_LEASE" => Some(Capability::CAP_LEASE),
            "CAP_LINUX_IMMUTABLE" => Some(Capability::CAP_LINUX_IMMUTABLE),
            "CAP_MAC_ADMIN" => Some(Capability::CAP_MAC_ADMIN),
            "CAP_MAC_OVERRIDE" => Some(Capability::CAP_MAC_OVERRIDE),
            "CAP_MKNOD" => Some(Capability::CAP_MKNOD),
            "CAP_NET_ADMIN" => Some(Capability::CAP_NET_ADMIN),
            "CAP_NET_BIND_SERVICE" => Some(Capability::CAP_NET_BIND_SERVICE),
            "CAP_NET_BROADCAST" => Some(Capability::CAP_NET_BROADCAST),
            "CAP_NET_RAW" => Some(Capability::CAP_NET_RAW),
            "CAP_SETGID" => Some(Capability::CAP_SETGID),
            "CAP_SETFCAP" => Some(Capability::CAP_SETFCAP),
            "CAP_SETPCAP" => Some(Capability::CAP_SETPCAP),
            "CAP_SETUID" => Some(Capability::CAP_SETUID),
            "CAP_SYSLOG" => Some(Capability::CAP_SYSLOG),
            "CAP_SYS_ADMIN" => Some(Capability::CAP_SYS_ADMIN),
            "CAP_SYS_BOOT" => Some(Capability::CAP_SYS_BOOT),
            "CAP_SYS_CHROOT" => Some(Capability::CAP_SYS_CHROOT),
            "CAP_SYS_MODULE" => Some(Capability::CAP_SYS_MODULE),
            "CAP_SYS_NICE" => Some(Capability::CAP_SYS_NICE),
            "CAP_SYS_PACCT" => Some(Capability::CAP_SYS_PACCT),
            "CAP_SYS_PTRACE" => Some(Capability::CAP_SYS_PTRACE),
            "CAP_SYS_RAWIO" => Some(Capability::CAP_SYS_RAWIO),
            "CAP_SYS_RESOURCE" => Some(Capability::CAP_SYS_RESOURCE),
            "CAP_SYS_TIME" => Some(Capability::CAP_SYS_TIME),
            "CAP_SYS_TTY_CONFIG" => Some(Capability::CAP_SYS_TTY_CONFIG),
            "CAP_WAKE_ALARM" => Some(Capability::CAP_WAKE_ALARM),
            _ => None,
        }
    }
}

/// Get the default capabilities to keep for containers.
///
/// These are capabilities that most containers need for basic operation.
/// All other capabilities should be dropped.
pub fn get_default_caps() -> HashSet<Capability> {
    HashSet::from_iter(vec![
        // Basic file operations
        Capability::CAP_CHOWN,
        Capability::CAP_DAC_OVERRIDE,
        Capability::CAP_FOWNER,
        Capability::CAP_FSETID,
        // Identity management
        Capability::CAP_SETGID,
        Capability::CAP_SETUID,
        // Basic IPC
        Capability::CAP_IPC_LOCK,
        Capability::CAP_KILL,
        Capability::CAP_SETFCAP,
        // Network binding (web servers, etc.)
        Capability::CAP_NET_BIND_SERVICE,
        // Resource management
        Capability::CAP_SYS_CHROOT,
    ])
}

/// Get capabilities for a minimal security profile.
///
/// Keeps only the most essential capabilities for strict isolation.
pub fn get_minimal_caps() -> HashSet<Capability> {
    HashSet::from_iter(vec![
        Capability::CAP_SETUID,
        Capability::CAP_SETGID,
    ])
}

/// Get capabilities for a web server container.
///
/// Includes capabilities commonly needed by web servers.
pub fn get_web_caps() -> HashSet<Capability> {
    HashSet::from_iter(vec![
        Capability::CAP_CHOWN,
        Capability::CAP_DAC_OVERRIDE,
        Capability::CAP_FOWNER,
        Capability::CAP_FSETID,
        Capability::CAP_SETGID,
        Capability::CAP_SETUID,
        Capability::CAP_NET_BIND_SERVICE,
    ])
}

/// Get capabilities to drop (dangerous ones).
///
/// These capabilities should typically be dropped for security.
pub fn get_caps_to_drop() -> HashSet<Capability> {
    let all = Capability::all();
    let default = get_default_caps();
    all.difference(&default).cloned().collect()
}

/// Drop all capabilities except those specified.
///
/// This is the primary function for capability management.
/// It keeps only the capabilities in the `keep` set and drops all others.
///
/// # Arguments
///
/// * `keep` - Set of capabilities to retain
///
/// # Example
///
/// ```no_run
/// use exo_runtime::security::{drop_capabilities, Capability};
///
/// // Keep only basic capabilities
/// let keep = vec![
///     Capability::CAP_SETUID,
///     Capability::CAP_SETGID,
/// ];
/// drop_capabilities(&keep)?;
/// ```
#[cfg(target_os = "linux")]
pub fn drop_capabilities(keep: &[Capability]) -> Result<()> {
    let keep_set: HashSet<CapsCapability> = keep.iter()
        .map(|c| c.as_caps_capability())
        .collect();

    // Get all capabilities currently in the effective set
    let current = caps::read(None, CapSet::Effective)
        .context("Failed to read current capabilities")?;

    // Drop capabilities not in keep set
    for cap in current {
        if !keep_set.contains(&cap) {
            caps::drop(None, CapSet::Effective, cap)
                .with_context(|| format!("Failed to drop capability: {:?}", cap))?;
            tracing::debug!("Dropped capability: {:?}", cap);
        }
    }

    // Also drop from permitted set
    let permitted = caps::read(None, CapSet::Permitted)
        .context("Failed to read permitted capabilities")?;

    for cap in permitted {
        if !keep_set.contains(&cap) {
            let _ = caps::drop(None, CapSet::Permitted, cap);
        }
    }

    tracing::info!("Dropped capabilities, kept: {:?}", keep);

    Ok(())
}

/// Raise specific capabilities.
///
/// Add capabilities to the effective and permitted sets.
/// This is typically used in privileged mode.
///
/// # Arguments
///
/// * `caps` - Capabilities to raise
#[cfg(target_os = "linux")]
pub fn raise_capabilities(caps: &[Capability]) -> Result<()> {
    for cap in caps {
        let caps_cap = cap.as_caps_capability();

        caps::raise(None, CapSet::Effective, caps_cap)
            .with_context(|| format!("Failed to raise capability: {}", cap.as_str()))?;

        tracing::debug!("Raised capability: {}", cap.as_str());
    }

    tracing::info!("Raised capabilities: {:?}", caps);

    Ok(())
}

/// Drop all capabilities (strict security mode).
///
/// Removes all capabilities from the effective set.
#[cfg(target_os = "linux")]
pub fn drop_all_capabilities() -> Result<()> {
    drop_capabilities(&[])
}

/// Reset capabilities to a specific set.
///
/// Drops all capabilities then raises the specified ones.
#[cfg(target_os = "linux")]
pub fn reset_capabilities(caps: &[Capability]) -> Result<()> {
    // First drop all
    drop_all_capabilities()?;

    // Then raise specified ones
    raise_capabilities(caps)?;

    Ok(())
}

/// Get current capabilities for introspection.
#[cfg(target_os = "linux")]
pub fn get_current_caps() -> Result<HashSet<Capability>> {
    let effective = caps::read(None, CapSet::Effective)
        .context("Failed to read effective capabilities")?;

    let mut result = HashSet::new();

    for cap in effective {
        // Map from caps crate Capability to our enum
        let name = format!("{:?}", cap);
        if let Some(our_cap) = Capability::from_str(&name) {
            result.insert(our_cap);
        }
    }

    Ok(result)
}

/// Check if running with privileges (has CAP_SYS_ADMIN).
#[cfg(target_os = "linux")]
pub fn has_privileged_caps() -> bool {
    match caps::has_cap(None, CapSet::Effective, CapsCapability::CAP_SYS_ADMIN) {
        Ok(true) => true,
        _ => false,
    }
}

/// Non-Linux stub implementations
#[cfg(not(target_os = "linux"))]
pub fn drop_capabilities(_keep: &[Capability]) -> Result<()> {
    tracing::warn!("Capability dropping not supported on this platform");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn raise_capabilities(_caps: &[Capability]) -> Result<()> {
    tracing::warn!("Capability raising not supported on this platform");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn drop_all_capabilities() -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn reset_capabilities(_caps: &[Capability]) -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn get_current_caps() -> Result<HashSet<Capability>> {
    Ok(HashSet::new())
}

#[cfg(not(target_os = "linux"))]
pub fn has_privileged_caps() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_names() {
        assert_eq!(Capability::CAP_NET_RAW.as_str(), "CAP_NET_RAW");
        assert_eq!(Capability::CAP_SYS_ADMIN.as_str(), "CAP_SYS_ADMIN");
    }

    #[test]
    fn test_capability_parsing() {
        assert_eq!(
            Capability::from_str("CAP_NET_RAW"),
            Some(Capability::CAP_NET_RAW)
        );
        assert_eq!(
            Capability::from_str("INVALID"),
            None
        );
    }

    #[test]
    fn test_default_caps() {
        let defaults = get_default_caps();
        assert!(defaults.contains(&Capability::CAP_SETUID));
        assert!(defaults.contains(&Capability::CAP_SETGID));
        assert!(defaults.contains(&Capability::CAP_NET_BIND_SERVICE));
    }

    #[test]
    fn test_minimal_caps() {
        let minimal = get_minimal_caps();
        assert!(minimal.contains(&Capability::CAP_SETUID));
        assert!(minimal.contains(&Capability::CAP_SETGID));
        assert!(!minimal.contains(&Capability::CAP_NET_ADMIN));
    }

    #[test]
    fn test_caps_to_drop() {
        let to_drop = get_caps_to_drop();
        // These dangerous capabilities should be in the drop list
        assert!(to_drop.contains(&Capability::CAP_NET_RAW));
        assert!(to_drop.contains(&Capability::CAP_NET_ADMIN));
        assert!(to_drop.contains(&Capability::CAP_SYS_ADMIN));
        // CAP_SYS_CHROOT is in default caps to keep, so should NOT be in drop list
        assert!(!to_drop.contains(&Capability::CAP_SYS_CHROOT));
        assert!(to_drop.contains(&Capability::CAP_SYS_MODULE));
    }

    #[test]
    fn test_web_caps() {
        let web = get_web_caps();
        assert!(web.contains(&Capability::CAP_NET_BIND_SERVICE));
    }
}
