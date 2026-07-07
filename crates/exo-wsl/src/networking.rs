//! Networking support for Exo containers.
//!
//! Simplified networking focused on AI agent communication via Tailnet.
//! Since Tailnet handles service discovery, we just need:
//! - Bridge network setup
//! - Port mapping from host to container
//! - Static IP assignment

use crate::WslConfig;
use anyhow::Result;
use std::process::Command;
use tracing::{debug, info, warn};

/// Network configuration for a container.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Network mode: bridge, host, or none
    pub mode: NetworkMode,
    /// Bridge name
    pub bridge_name: String,
    /// Container static IP (e.g., "172.17.0.2")
    pub container_ip: Option<String>,
    /// Gateway IP
    pub gateway_ip: Option<String>,
    /// Subnet mask (e.g., "255.255.255.0")
    pub subnet_mask: Option<String>,
    /// Port mappings: host_port -> container_port
    pub port_mappings: Vec<PortMapping>,
}

/// Network mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    /// Bridge network - containers on isolated network
    Bridge,
    /// Host network - shares host network namespace
    Host,
    /// No network - completely isolated
    None,
}

/// Port mapping configuration.
#[derive(Debug, Clone)]
pub struct PortMapping {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: PortProtocol,
    pub host_ip: String,
}

/// Port protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortProtocol {
    Tcp,
    Udp,
    Both,
}

impl PortProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            PortProtocol::Tcp => "TCP",
            PortProtocol::Udp => "UDP",
            PortProtocol::Both => "TCP", // Default to TCP for "both"
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            mode: NetworkMode::Bridge,
            bridge_name: "exo-br0".to_string(),
            container_ip: None,
            gateway_ip: Some("172.17.0.1".to_string()),
            subnet_mask: Some("255.255.255.0".to_string()),
            port_mappings: vec![],
        }
    }
}

/// Network manager for creating and managing container networks.
pub struct NetworkManager {
    bridge_name: String,
    subnet: String,
    gateway_ip: String,
    distro_name: String,
}

impl NetworkManager {
    /// Create a new network manager.
    pub fn new(bridge_name: &str, subnet: &str, gateway_ip: &str) -> Result<Self> {
        Ok(Self {
            bridge_name: bridge_name.to_string(),
            subnet: subnet.to_string(),
            gateway_ip: gateway_ip.to_string(),
            distro_name: WslConfig::default().distro_name,
        })
    }

    /// Create a new network manager with a specific distro.
    pub fn with_distro(
        bridge_name: &str,
        subnet: &str,
        gateway_ip: &str,
        distro: &str,
    ) -> Result<Self> {
        Ok(Self {
            bridge_name: bridge_name.to_string(),
            subnet: subnet.to_string(),
            gateway_ip: gateway_ip.to_string(),
            distro_name: distro.to_string(),
        })
    }

    /// Create a new network manager with full config.
    pub fn from_config(
        config: &WslConfig,
        bridge_name: &str,
        subnet: &str,
        gateway_ip: &str,
    ) -> Result<Self> {
        Ok(Self {
            bridge_name: bridge_name.to_string(),
            subnet: subnet.to_string(),
            gateway_ip: gateway_ip.to_string(),
            distro_name: config.distro_name.clone(),
        })
    }

    /// Create the bridge network for containers.
    /// In WSL2, this may not work due to limited capabilities, so we fall back gracefully.
    pub fn create_bridge(&self) -> Result<()> {
        // Check if bridge already exists
        if self.bridge_exists()? {
            debug!("Bridge {} already exists", self.bridge_name);
            return Ok(());
        }

        info!("Attempting to create bridge: {}", self.bridge_name);

        // Try to create bridge - this may fail in WSL2 due to limited capabilities
        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.distro_name,
                "--",
                "bash",
                "-c",
                &format!(
                    "ip link add name {} type bridge && ip addr add {} dev {} && ip link set {} up",
                    self.bridge_name, self.subnet, self.bridge_name, self.bridge_name
                ),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Could not create bridge (this is expected in WSL2): {}. Containers will use host networking.", stderr.trim());
            // Don't fail - just log and continue. Containers will fall back to host networking.
            return Ok(());
        }

        // Enable IP forwarding
        self.enable_ip_forwarding()?;

        info!("Bridge {} created successfully", self.bridge_name);
        Ok(())
    }

    /// Check if the bridge exists.
    fn bridge_exists(&self) -> Result<bool> {
        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.distro_name,
                "--",
                "ip",
                "link",
                "show",
                &self.bridge_name,
            ])
            .output()?;

        Ok(output.status.success())
    }

    /// Enable IP forwarding for container networking.
    fn enable_ip_forwarding(&self) -> Result<()> {
        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.distro_name,
                "--",
                "bash",
                "-c",
                "sysctl -w net.ipv4.ip_forward=1",
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to enable IP forwarding: {}", stderr);
        }

        Ok(())
    }

    /// Set up networking for a specific container.
    /// In WSL2, falls back to simplified networking if bridge creation fails.
    pub fn setup_container_network(
        &self,
        container_name: &str,
        config: &NetworkConfig,
    ) -> Result<ContainerNetwork> {
        let veth_host = format!("{}-veth", container_name);
        let veth_container = "eth0";
        let ip = config
            .container_ip
            .clone()
            .unwrap_or_else(|| self.generate_ip());

        // Check if bridge exists - if not, use simplified networking
        if !self.bridge_exists().unwrap_or(false) {
            info!(
                "Bridge not available, using simplified networking for container {}",
                container_name
            );

            // For WSL2 without bridge, we return a minimal network config
            // The container will use the default WSL2 networking
            return Ok(ContainerNetwork {
                veth_host: "host".to_string(),
                veth_container: "eth0".to_string(),
                bridge_name: "wsl".to_string(),
                ip: "127.0.0.1".to_string(), // Use localhost for WSL2
            });
        }

        // Create veth pair (host side in WSL2, container side inside container)
        if let Err(e) = self.create_veth_pair(&veth_host, veth_container) {
            warn!(
                "Failed to create veth pair: {}. Using simplified networking.",
                e
            );
            return Ok(ContainerNetwork {
                veth_host: "host".to_string(),
                veth_container: "eth0".to_string(),
                bridge_name: "wsl".to_string(),
                ip: "127.0.0.1".to_string(),
            });
        }

        // Attach veth host to bridge
        if let Err(e) = self.attach_to_bridge(&veth_host) {
            warn!(
                "Failed to attach to bridge: {}. Using simplified networking.",
                e
            );
            return Ok(ContainerNetwork {
                veth_host: "host".to_string(),
                veth_container: "eth0".to_string(),
                bridge_name: "wsl".to_string(),
                ip: "127.0.0.1".to_string(),
            });
        }

        // Set up port mappings
        for mapping in &config.port_mappings {
            self.setup_port_mapping(mapping)?;
        }

        Ok(ContainerNetwork {
            veth_host: veth_host.clone(),
            veth_container: veth_container.to_string(),
            bridge_name: self.bridge_name.clone(),
            ip: ip.clone(),
        })
    }

    /// Create a veth pair for container networking.
    fn create_veth_pair(&self, veth_host: &str, veth_container: &str) -> Result<()> {
        info!("Creating veth pair: {} <-> {}", veth_host, veth_container);

        let output = Command::new("wsl")
            .args(["--distribution", &self.distro_name, "--", "bash", "-c",
                &format!("ip link add {} {} type veth peer name {} && ip link set {} up && ip link set {} up",
                    veth_host, veth_host, veth_container, veth_host, veth_container)])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create veth pair: {}", stderr);
        }

        Ok(())
    }

    /// Attach the host veth to the bridge.
    fn attach_to_bridge(&self, veth_host: &str) -> Result<()> {
        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.distro_name,
                "--",
                "bash",
                "-c",
                &format!("ip link set master {} dev {}", self.bridge_name, veth_host),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to attach {} to bridge: {}", veth_host, stderr);
        }

        debug!("Attached {} to bridge {}", veth_host, self.bridge_name);

        Ok(())
    }

    /// Set up port forwarding from host to container.
    fn setup_port_mapping(&self, mapping: &PortMapping) -> Result<()> {
        let protocol = match mapping.protocol {
            PortProtocol::Tcp => "tcp",
            PortProtocol::Udp => "udp",
            PortProtocol::Both => "all",
        };

        let output = Command::new("wsl")
            .args(["--distribution", &self.distro_name, "--", "bash", "-c",
                &format!("iptables -t nat -A PREROUTING -p {} --destination 127.0.0.1:{} -j DNAT --to-destination {}:{}",
                    protocol, mapping.host_port, mapping.container_port, mapping.container_port)])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to set up port mapping: {}", stderr);
        }

        info!(
            "Port mapping: {}:{} -> {}:{} ({})",
            mapping.host_port,
            mapping.container_port,
            mapping.host_ip,
            mapping.container_port,
            protocol
        );

        Ok(())
    }

    /// Clean up networking for a stopped container.
    pub fn cleanup_container_network(&self, container_name: &str) -> Result<()> {
        let veth_host = format!("{}-veth", container_name);

        // Delete port forwarding rules
        // TODO: Track rules to remove them properly

        // Delete veth pair
        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.distro_name,
                "--",
                "ip",
                "link",
                "delete",
                &veth_host,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to delete veth {}: {}", veth_host, stderr);
        } else {
            info!("Deleted veth {}", veth_host);
        }

        Ok(())
    }

    /// Generate a static IP for a container.
    fn generate_ip(&self) -> String {
        // Simple sequential IP assignment: 172.17.0.{2, 3, 4...}
        // In production, track used IPs
        "172.17.0.2".to_string()
    }

    /// Clean up the bridge when shutting down.
    pub fn cleanup_bridge(&self) -> Result<()> {
        let output = Command::new("wsl")
            .args([
                "--distribution",
                &self.distro_name,
                "--",
                "ip",
                "link",
                "delete",
                &self.bridge_name,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to delete bridge {}: {}", self.bridge_name, stderr);
        } else {
            info!("Deleted bridge {}", self.bridge_name);
        }

        Ok(())
    }
}

/// Network information for a running container.
#[derive(Debug, Clone)]
pub struct ContainerNetwork {
    pub veth_host: String,
    pub veth_container: String,
    pub bridge_name: String,
    pub ip: String,
}

/// Simple DNS entries for container-to-container communication.
#[derive(Debug, Clone)]
pub struct DnsEntry {
    pub name: String,
    pub ip: String,
}

impl DnsEntry {
    /// Create a new DNS entry.
    pub fn new(name: &str, ip: &str) -> Self {
        Self {
            name: name.to_string(),
            ip: ip.to_string(),
        }
    }

    /// Convert to hosts file format.
    pub fn to_hosts_entry(&self) -> String {
        format!("{}\t{}", self.ip, self.name)
    }

    /// Write to container's /etc/hosts file.
    pub fn write_to_container(&self, container_state_dir: &str) -> Result<()> {
        let hosts_path = format!("{}/etc/hosts", container_state_dir);
        std::fs::create_dir_all(format!("{}/etc", container_state_dir))?;

        let hosts = format!("127.0.0.1 localhost\n{}\n", self.to_hosts_entry());

        std::fs::write(hosts_path, hosts)?;
        debug!("Wrote hosts file for {}", self.name);

        Ok(())
    }
}

/// Configuration for inter-agent networking.
#[derive(Debug, Clone)]
pub struct AgentNetworkConfig {
    pub subnet: String,
    pub bridge_name: String,
    pub gateway_ip: String,
    pub base_ip: String,
}

impl Default for AgentNetworkConfig {
    fn default() -> Self {
        Self {
            subnet: "172.17.0.0/24".to_string(),
            bridge_name: "exo-br0".to_string(),
            gateway_ip: "172.17.0.1".to_string(),
            base_ip: "172.17.0".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_config_default() {
        let config = NetworkConfig::default();
        assert_eq!(config.mode, NetworkMode::Bridge);
        assert_eq!(config.bridge_name, "exo-br0");
    }

    #[test]
    fn test_dns_entry() {
        let entry = DnsEntry::new("agent-1", "172.17.0.2");
        assert_eq!(entry.to_hosts_entry(), "172.17.0.2\tagent-1");
    }
}
