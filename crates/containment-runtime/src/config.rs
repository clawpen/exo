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
    "openclaw".to_string()
}

fn default_namespaces() -> Namespaces {
    Namespaces {
        pid: true,
        network: true,
        ipc: true,
        uts: true,
        mount: true,
        user: true,
        cgroup: true,
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
        let config = serde_json::json!({
            "name": "test",
            "image": "python:3.12",
            "command": ["python", "app.py"]
        });

        let cfg: ContainerConfig = serde_json::from_value(config).unwrap();
        assert_eq!(cfg.workdir, PathBuf::from("/app"));
        assert_eq!(cfg.user, "root");
        assert_eq!(cfg.hostname, "openclaw");
    }
}
