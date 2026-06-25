//! Container configuration types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Complete container configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerConfig {
    /// Container name
    pub name: String,

    /// Container image (OCI reference)
    pub image: String,

    /// Working directory inside the container
    #[serde(default = "default_workdir")]
    pub workdir: PathBuf,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// User to run as (uid:gid or username)
    #[serde(default = "default_user")]
    pub user: String,

    /// Command to run
    pub command: Vec<String>,

    /// Resource limits
    #[serde(default)]
    pub resources: ResourceConfig,

    /// Network configuration
    #[serde(default)]
    pub network: NetworkConfig,

    /// Mounts/bind mounts
    #[serde(default)]
    pub mounts: Vec<MountConfig>,

    /// GPU configuration
    #[serde(default)]
    pub gpu: Option<GpuConfig>,

    /// Namespaces to isolate
    #[serde(default = "default_namespaces")]
    pub namespaces: Namespaces,

    /// Hostname for the container
    #[serde(default = "default_hostname")]
    pub hostname: String,

    /// Whether to use privileged mode
    #[serde(default)]
    pub privileged: bool,

    /// Read-only root filesystem
    #[serde(default)]
    pub readonly_rootfs: bool,

    /// Target architecture (auto-detected from image if not specified)
    #[serde(default)]
    pub architecture: Option<String>,

    /// Platform in OCI format (os/arch variant)
    #[serde(default)]
    pub platform: Option<String>,

    /// What the reconciler should do if this container's process is gone.
    /// Defaults to Never (record exit, no restart).
    #[serde(default)]
    pub restart_policy: RestartPolicy,

    /// Optional list of overlay lowerdir paths. When present and whiteout-free,
    /// the runtime mounts these layer directories directly instead of building a
    /// per-image hardlink-composed rootfs.
    #[serde(default)]
    pub overlay_lowerdirs: Option<Vec<PathBuf>>,
}

/// When the reconciler should re-spawn a container whose process has died.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    /// Do not restart (Docker-compatible `no`).
    #[default]
    #[serde(alias = "never")]
    No,
    /// Restart only on non-zero exit (Docker-compatible `on-failure`).
    OnFailure,
    /// Always restart (Docker-compatible `always`).
    Always,
    /// Restart only when discovered during the daemon's startup recovery pass
    /// (i.e., the daemon itself died — not the container exiting on its own).
    OnDaemonRestart,
}

impl RestartPolicy {
    /// Parse from Docker-style strings (`no`, `on-failure`, `always`).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "no" | "never" => Some(RestartPolicy::No),
            "on-failure" => Some(RestartPolicy::OnFailure),
            "always" => Some(RestartPolicy::Always),
            "on-daemon-restart" | "on_daemon_restart" => Some(RestartPolicy::OnDaemonRestart),
            _ => None,
        }
    }
}

impl Default for ContainerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: String::new(),
            workdir: default_workdir(),
            env: HashMap::new(),
            user: default_user(),
            command: vec![],
            resources: ResourceConfig::default(),
            network: NetworkConfig::default(),
            mounts: vec![],
            gpu: None,
            namespaces: Namespaces::default(),
            hostname: default_hostname(),
            privileged: false,
            readonly_rootfs: false,
            architecture: None,
            platform: None,
            restart_policy: RestartPolicy::default(),
            overlay_lowerdirs: None,
        }
    }
}

fn default_workdir() -> PathBuf {
    PathBuf::from("/app")
}

fn default_user() -> String {
    "root".to_string()
}

fn default_hostname() -> String {
    "containment".to_string()
}

fn default_namespaces() -> Namespaces {
    // Default for rootless: user + mount + uts + ipc
    // Network and cgroup require privileges
    // PID namespace DISABLED: causes hangs with node.js processes in WSL2
    Namespaces {
        pid: false,      // DISABLED: double-fork hangs with node.js
        network: false,  // Requires CAP_NET_ADMIN
        ipc: true,       // Works in user namespace
        uts: true,       // Works in user namespace
        mount: true,     // Works in user namespace
        user: true,      // Core of rootless containers
        cgroup: false,   // Requires CAP_SYS_ADMIN
    }
}

/// Namespace configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespaces {
    /// PID namespace (isolate process IDs)
    pub pid: bool,

    /// Network namespace (isolate network stack)
    pub network: bool,

    /// IPC namespace (isolate IPC mechanisms)
    pub ipc: bool,

    /// UTS namespace (isolate hostname and domain)
    pub uts: bool,

    /// Mount namespace (isolate filesystem mounts)
    pub mount: bool,

    /// User namespace (isolate user IDs)
    pub user: bool,

    /// Cgroup namespace (isolate cgroups)
    pub cgroup: bool,
}

impl Default for Namespaces {
    fn default() -> Self {
        default_namespaces()
    }
}

/// Resource limits configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConfig {
    /// Memory limit (e.g., "2G", "512M")
    pub memory: Option<String>,

    /// CPU count or percentage (e.g., "2", "200%")
    pub cpu: Option<String>,

    /// Specific CPU cores to use (e.g., "0,1" or "0-3")
    pub cpus: Option<String>,

    /// Memory swap limit
    pub memory_swap: Option<String>,

    /// Memory reservation (soft limit)
    pub memory_reservation: Option<String>,

    /// CPU shares (relative weight)
    pub cpu_shares: Option<u64>,

    /// PIDs limit (max processes)
    pub pids_limit: Option<u64>,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            memory: None,
            cpu: None,
            cpus: None,
            memory_swap: None,
            memory_reservation: None,
            cpu_shares: None,
            pids_limit: None,
        }
    }
}

/// Network configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Network mode: bridge, host, none, container:<id>
    #[serde(default = "default_network_mode")]
    pub mode: String,

    /// Port mappings: host_port:container_port
    #[serde(default)]
    pub port_mappings: Vec<PortMapping>,

    /// DNS servers
    #[serde(default)]
    pub dns: Vec<String>,

    /// Network name (for bridge mode)
    #[serde(default = "default_network_name")]
    pub network_name: String,

    /// IP address to assign
    pub ip_address: Option<String>,

    /// Gateway address
    pub gateway: Option<String>,

    /// Hostname to publish
    pub hostname: Option<String>,
}

fn default_network_mode() -> String {
    "bridge".to_string()
}

fn default_network_name() -> String {
    "openclaw0".to_string()
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            mode: default_network_mode(),
            port_mappings: vec![],
            dns: vec![],
            network_name: default_network_name(),
            ip_address: None,
            gateway: None,
            hostname: None,
        }
    }
}

/// Port mapping configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortMapping {
    /// Host port
    pub host_port: u16,

    /// Container port
    pub container_port: u16,

    /// Protocol: tcp or udp
    #[serde(default = "default_protocol")]
    pub protocol: String,

    /// Host IP to bind to
    #[serde(default)]
    pub host_ip: String,
}

fn default_protocol() -> String {
    "tcp".to_string()
}

/// Mount configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    /// Mount type: bind, volume, tmpfs
    #[serde(default = "default_mount_type")]
    pub mount_type: String,

    /// Source path (on host or volume name)
    pub source: String,

    /// Destination path (in container)
    pub target: String,

    /// Read-only mount
    #[serde(default)]
    pub readonly: bool,

    /// For tmpfs: size limit
    pub size: Option<String>,

    /// Propagation mode: rprivate, rshared, etc.
    #[serde(default)]
    pub propagation: String,
}

fn default_mount_type() -> String {
    "bind".to_string()
}

/// GPU configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuConfig {
    /// GPU type: nvidia, amd, auto
    #[serde(default = "default_gpu_type")]
    pub gpu_type: String,

    /// GPU devices to passthrough (e.g., ["all"], ["0", "1"])
    #[serde(default = "default_gpu_devices")]
    pub devices: Vec<String>,

    /// Compute mode (NVIDIA-specific)
    pub compute_mode: Option<String>,
}

fn default_gpu_type() -> String {
    "auto".to_string()
}

fn default_gpu_devices() -> Vec<String> {
    vec!["all".to_string()]
}

/// Parse a size string like "2G" or "512M" into bytes.
pub fn parse_size(size: &str) -> anyhow::Result<u64> {
    let size = size.trim().to_lowercase();

    let (num, unit) = if size.ends_with('b') {
        let s = size.trim_end_matches('b');
        if s.ends_with('g') {
            (s.trim_end_matches('g'), "g")
        } else if s.ends_with('m') {
            (s.trim_end_matches('m'), "m")
        } else if s.ends_with('k') {
            (s.trim_end_matches('k'), "k")
        } else {
            (s, "b")
        }
    } else if size.ends_with('g') {
        (size.trim_end_matches('g'), "g")
    } else if size.ends_with('m') {
        (size.trim_end_matches('m'), "m")
    } else if size.ends_with('k') {
        (size.trim_end_matches('k'), "k")
    } else {
        (&size[..], "b")
    };

    let num: u64 = num.parse().map_err(|_| anyhow::anyhow!("Invalid size: {}", size))?;

    Ok(match unit {
        "g" => num * 1024 * 1024 * 1024,
        "m" => num * 1024 * 1024,
        "k" => num * 1024,
        _ => num,
    })
}

impl ContainerConfig {
    /// Detect architecture from image name (e.g., "arm64v8/python", "python:arm64")
    pub fn detect_architecture(&self) -> Option<&str> {
        // Check explicit architecture field first
        if let Some(ref arch) = self.architecture {
            return Some(arch);
        }

        // Check platform field
        if let Some(ref platform) = self.platform {
            if let Some(arch_part) = platform.split('/').nth(1) {
                return Some(arch_part);
            }
        }

        // Parse image name for architecture hints
        let image_lower = self.image.to_lowercase();

        // Common architecture prefixes in Docker images
        let arch_patterns = [
            ("arm64v8/", "aarch64"),
            ("arm64v8-", "aarch64"),
            ("arm64v8", "aarch64"),
            ("arm64/", "aarch64"),
            ("armv7/", "arm"),
            ("armv7-", "arm"),
            ("armv7", "arm"),
            ("arm32v7/", "arm"),
            ("arm32v7-", "arm"),
            ("ppc64le/", "ppc64le"),
            ("s390x/", "s390x"),
            ("riscv64/", "riscv64"),
        ];

        for (pattern, arch) in &arch_patterns {
            if image_lower.contains(pattern) {
                return Some(arch);
            }
        }

        // Check tag for architecture suffix
        if let Some(tag_part) = self.image.split(':').nth(1) {
            let tag_lower = tag_part.to_lowercase();
            if tag_lower.contains("arm64") || tag_lower.contains("aarch64") {
                return Some("aarch64");
            }
            if tag_lower.contains("armv7") || tag_lower.contains("arm32") {
                return Some("arm");
            }
            if tag_lower.contains("ppc64le") {
                return Some("ppc64le");
            }
            if tag_lower.contains("s390x") {
                return Some("s390x");
            }
            if tag_lower.contains("riscv64") {
                return Some("riscv64");
            }
        }

        None
    }

    /// Check if this container requires foreign binary execution
    pub fn requires_foreign_exec(&self) -> bool {
        if let Some(detected_arch) = self.detect_architecture() {
            #[cfg(target_os = "linux")]
            {
                use crate::binfmt::Architecture;
                if let Some(arch) = Architecture::from_str(detected_arch) {
                    return arch.is_foreign();
                }
            }
        }
        false
    }

    /// Get the target architecture as a string
    pub fn target_arch(&self) -> String {
        self.detect_architecture()
            .unwrap_or_else(|| {
                // cfg!() if/else avoids the unreachable_code warning that
                // cfg-attributed `return`s produce on the matching arch.
                if cfg!(target_arch = "aarch64") {
                    "aarch64"
                } else if cfg!(target_arch = "arm") {
                    "arm"
                } else if cfg!(target_arch = "riscv64") {
                    "riscv64"
                } else if cfg!(target_arch = "powerpc64") {
                    "ppc64le"
                } else {
                    "x86_64"
                }
            })
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("512M").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_size("1024K").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }

    #[test]
    fn test_container_config_default() {
        let config = ContainerConfig::default();
        assert_eq!(config.workdir, PathBuf::from("/app"));
        assert_eq!(config.user, "root");
        assert_eq!(config.hostname, "containment");
        assert!(!config.privileged);
        assert!(!config.readonly_rootfs);
        assert!(config.env.is_empty());
        assert!(config.mounts.is_empty());
        assert!(config.gpu.is_none());
    }

    #[test]
    fn test_resource_config_default() {
        let config = ResourceConfig::default();
        assert!(config.memory.is_none());
        assert!(config.cpu.is_none());
        assert!(config.cpu_shares.is_none());
        assert!(config.pids_limit.is_none());
    }

    #[test]
    fn test_network_config_default() {
        let config = NetworkConfig::default();
        assert_eq!(config.mode, "bridge");
        assert!(config.port_mappings.is_empty());
        assert!(config.dns.is_empty());
    }

    #[test]
    fn test_namespaces_default() {
        let ns = Namespaces::default();
        assert!(ns.user);     // Core of rootless containers
        assert!(!ns.pid);     // DISABLED: hangs with node.js in WSL2
        assert!(!ns.network); // Requires CAP_NET_ADMIN
        assert!(ns.mount);    // Works in user namespace
        assert!(ns.uts);      // Works in user namespace
        assert!(ns.ipc);      // Works in user namespace
        assert!(!ns.cgroup);  // Requires CAP_SYS_ADMIN
    }

    #[test]
    fn test_gpu_config_default() {
        let config = GpuConfig {
            gpu_type: "auto".to_string(),
            devices: vec!["all".to_string()],
            compute_mode: None,
        };
        assert_eq!(config.gpu_type, "auto");
        assert!(!config.devices.is_empty());
    }

    #[test]
    fn test_container_config_with_env() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
        env.insert("HOME".to_string(), "/root".to_string());

        let mut config = ContainerConfig::default();
        config.env = env;

        assert_eq!(config.env.get("PATH"), Some(&"/usr/bin:/bin".to_string()));
        assert_eq!(config.env.get("HOME"), Some(&"/root".to_string()));
    }

    #[test]
    fn test_container_config_serialization() {
        let config = ContainerConfig {
            name: "test".to_string(),
            image: "ubuntu:latest".to_string(),
            command: vec!["bash".to_string()],
            ..Default::default()
        };

        let json = serde_json::to_string(&config).expect("Failed to serialize");
        assert!(json.contains("test"));
        assert!(json.contains("ubuntu:latest"));

        let deserialized: ContainerConfig = serde_json::from_str(&json)
            .expect("Failed to deserialize");
        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.image, "ubuntu:latest");
    }

    #[test]
    fn test_mount_config() {
        let mount = MountConfig {
            mount_type: "bind".to_string(),
            source: "/host/path".to_string(),
            target: "/container/path".to_string(),
            readonly: false,
            size: None,
            propagation: "rprivate".to_string(),
        };

        assert_eq!(mount.source, "/host/path");
        assert_eq!(mount.target, "/container/path");
        assert!(!mount.readonly);
        assert_eq!(mount.mount_type, "bind");
    }
}
