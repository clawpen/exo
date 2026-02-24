//! Networking support for Containment containers.
//!
//! Simplified networking focused on AI agent communication via Tailnet.
//! Since Tailnet handles service discovery, we just need:
//! - Bridge network setup
//! - Port mapping from host to container
//! - Static IP assignment

use anyhow::Result;
use std::process::Command;
use tracing::{info, debug, warn};

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

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            mode: NetworkMode::Bridge,
            bridge_name: "containment-br0".to_string(),
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
}

impl NetworkManager {
    /// Create a new network manager.
    pub fn new(bridge_name: &str, subnet: &str, gateway_ip: &str) -> Result<Self> {
        Ok(Self {
            bridge_name: bridge_name.to_string(),
            subnet: subnet.to_string(),
            gateway_ip: gateway_ip.to_string(),
        })
    }

    /// Create the bridge network for containers.
    pub fn create_bridge(&self) -> Result<()> {
        // Check if bridge already exists
        if self.bridge_exists()? {
            debug!("Bridge {} already exists", self.bridge_name);
            return Ok(());
        }

        info!("Creating bridge: {}", self.bridge_name);

        // Create bridge
        let output = Command::new("wsl")
            .args(["--distribution", "containment", "--", "bash", "-c",
                &format!("ip link add name {} type bridge && ip addr add {} dev {} && ip link set {} up",
                    self.bridge_name, self.subnet, self.bridge_name, self.bridge_name)])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create bridge: {}", stderr);
        }

        // Enable IP forwarding
        self.enable_ip_forwarding()?;

        Ok(())
    }

    /// Check if the bridge exists.
    fn bridge_exists(&self) -> Result<bool> {
        let output = Command::new("wsl")
            .args(["--distribution", "containment", "--", "ip", "link", "show", &self.bridge_name])
            .output()?;

        Ok(output.status.success())
    }

    /// Enable IP forwarding for container networking.
    fn enable_ip_forwarding(&self) -> Result<()> {
        let output = Command::new("wsl")
            .args(["--distribution", "containment", "--", "bash", "-c",
                "sysctl -w net.ipv4.ip_forward=1"])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to enable IP forwarding: {}", stderr);
        }

        Ok(())
    }

    /// Set up networking for a specific container.
    pub fn setup_container_network(
        &self,
        container_name: &str,
        config: &NetworkConfig,
    ) -> Result<ContainerNetwork> {
        let veth_host = format!("{}-veth", container_name);
        let veth_container = "eth0";
        let ip = config.container_ip.clone()
            .unwrap_or_else(|| self.generate_ip());

        // Create veth pair (host side in WSL2, container side inside container)
        self.create_veth_pair(&veth_host, veth_container)?;

        // Attach veth host to bridge
        self.attach_to_bridge(&veth_host)?;

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
            .args(["--distribution", "containment", "--", "bash", "-c",
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
            .args(["--distribution", "containment", "--", "bash", "-c",
                &format!("ip link set master {} dev {}", self.bridge_name, veth_host)])
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
            .args(["--distribution", "containment", "--", "bash", "-c",
                &format!("iptables -t nat -A PREROUTING -p {} --destination 127.0.0.1:{} -j DNAT --to-destination {}:{}",
                    protocol, mapping.host_port, mapping.container_port, mapping.container_port)])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Failed to set up port mapping: {}", stderr);
        }

        info!("Port mapping: {}:{} -> {}:{} ({})",
            mapping.host_port, mapping.container_port, mapping.host_ip, mapping.container_port, protocol);

        Ok(())
    }

    /// Clean up networking for a stopped container.
    pub fn cleanup_container_network(&self, container_name: &str) -> Result<()> {
        let veth_host = format!("{}-veth", container_name);

        // Delete port forwarding rules
        // TODO: Track rules to remove them properly

        // Delete veth pair
        let output = Command::new("wsl")
            .args(["--distribution", "containment", "--", "ip", "link", "delete", &veth_host])
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
            .args(["--distribution", "containment", "--", "ip", "link", "delete", &self.bridge_name])
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
            bridge_name: "containment-br0".to_string(),
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
        assert_eq!(config.bridge_name, "containment-br0");
    }

    #[test]
    fn test_dns_entry() {
        let entry = DnsEntry::new("agent-1", "172.17.0.2");
        assert_eq!(entry.to_hosts_entry(), "172.17.0.2\tagent-1");
    }
}
