//! Network management for containers.
//!
//! This module handles port forwarding for rootless containers.
//! When containers run with isolated network namespaces, we need to
//! forward traffic from host ports to container ports.

use anyhow::{Context, Result};
use std::process::{Child, Command, Stdio};

/// Port forwarder using socat or similar tools.
pub struct PortForwarder {
    /// The socat/ncat process handling forwarding
    process: Option<Child>,
    /// Host port being listened on
    host_port: u16,
    /// Container port being forwarded to
    container_port: u16,
}

impl PortForwarder {
    /// Create a new port forwarder.
    pub fn new(host_port: u16, container_port: u16) -> Self {
        Self {
            process: None,
            host_port,
            container_port,
        }
    }

    /// Start port forwarding using socat.
    /// 
    /// For rootless containers, this listens on the host port and
    /// forwards connections to the container's loopback interface.
    /// Note: This works best when the container shares the host network namespace.
    /// For isolated network namespaces, more complex setup is needed.
    pub fn start(&mut self) -> Result<()> {
        // Check if socat is available
        if !is_command_available("socat") {
            tracing::warn!("socat not available, port forwarding may not work");
            // Try ncat as fallback
            return self.start_with_ncat();
        }

        let host_port = self.host_port;
        let container_port = self.container_port;

        // Use socat to forward TCP connections
        // Listen on host_port, forward to container_port on localhost
        let child = Command::new("socat")
            .arg(format!("TCP-LISTEN:{},fork,reuseaddr", host_port))
            .arg(format!("TCP:127.0.0.1:{}", container_port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start socat for port forwarding")?;

        self.process = Some(child);
        tracing::info!("Started port forwarding: {} -> {}", host_port, container_port);

        Ok(())
    }

    /// Try to start port forwarding using ncat (from nmap).
    fn start_with_ncat(&mut self) -> Result<()> {
        if !is_command_available("ncat") {
            tracing::warn!("ncat not available either, skipping port forwarding");
            return Ok(());
        }

        let host_port = self.host_port;
        let container_port = self.container_port;

        let child = Command::new("ncat")
            .arg("--listen")
            .arg("--keep-open")
            .arg(format!("{}", host_port))
            .arg("--sh-exec")
            .arg(format!("ncat 127.0.0.1 {}", container_port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start ncat for port forwarding")?;

        self.process = Some(child);
        tracing::info!("Started port forwarding with ncat: {} -> {}", host_port, container_port);

        Ok(())
    }

    /// Stop port forwarding.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("Stopped port forwarding: {} -> {}", self.host_port, self.container_port);
        }
        Ok(())
    }
}

impl Drop for PortForwarder {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Check if a command is available in PATH.
fn is_command_available(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Set up port forwarding for a container.
/// 
/// For containers with isolated network namespaces, this requires
/// the container's network namespace file descriptor to enter it.
/// For containers sharing the host network, no forwarding is needed.
/// 
/// # Arguments
/// * `port_mappings` - List of port mappings (host_port, container_port)
/// * `netns_path` - Path to the container's network namespace (e.g., /proc/PID/ns/net)
/// 
/// # Returns
/// Vector of PortForwarder handles that will clean up when dropped.
pub fn setup_port_forwarding(
    port_mappings: &[(u16, u16)],
    _netns_path: Option<&str>,
) -> Result<Vec<PortForwarder>> {
    let mut forwarders = Vec::new();

    for (host_port, container_port) in port_mappings {
        let mut forwarder = PortForwarder::new(*host_port, *container_port);
        forwarder.start()
            .with_context(|| format!("Failed to set up port forwarding {} -> {}", host_port, container_port))?;
        forwarders.push(forwarder);
    }

    Ok(forwarders)
}

/// Port forwarder that runs in the background and forwards to a specific PID's network namespace.
/// 
/// This is useful for rootless containers with isolated network namespaces.
pub struct NetnsPortForwarder {
    /// The socat process
    process: Option<Child>,
    /// Host port
    host_port: u16,
}

impl NetnsPortForwarder {
    /// Create a port forwarder that enters a specific network namespace.
    /// 
    /// Uses nsenter to enter the container's network namespace and then
    /// socat to forward traffic.
    pub fn new_for_netns(host_port: u16, container_port: u16, pid: u32) -> Result<Self> {
        // Check if nsenter and socat are available
        if !is_command_available("nsenter") {
            return Err(anyhow::anyhow!("nsenter not available for network namespace port forwarding"));
        }
        if !is_command_available("socat") {
            return Err(anyhow::anyhow!("socat not available for port forwarding"));
        }

        // Use nsenter to enter the container's network namespace
        // and run socat there to forward to the container's localhost
        let child = Command::new("nsenter")
            .arg(format!("--target={}", pid))
            .arg("--net")
            .arg("--")
            .arg("socat")
            .arg(format!("TCP-LISTEN:{},fork,reuseaddr", container_port))
            .arg(format!("TCP:127.0.0.1:{}", container_port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start nsenter+socat for port forwarding")?;

        tracing::info!("Started netns port forwarding: host {} -> container PID {} port {}", 
            host_port, pid, container_port);

        Ok(Self {
            process: Some(child),
            host_port,
        })
    }

    /// Stop the port forwarder.
    pub fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("Stopped netns port forwarding on port {}", self.host_port);
        }
        Ok(())
    }
}

impl Drop for NetnsPortForwarder {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Alternative: Use iptables for port forwarding (requires elevated privileges).
/// This is typically not available for rootless containers.
pub fn setup_iptables_forwarding(host_port: u16, container_ip: &str, container_port: u16) -> Result<()> {
    // DNAT rule for incoming traffic
    let status = Command::new("iptables")
        .args([
            "-t", "nat", "-A", "PREROUTING",
            "-p", "tcp",
            "--dport", &host_port.to_string(),
            "-j", "DNAT",
            "--to-destination", &format!("{}:{}", container_ip, container_port),
        ])
        .status()
        .context("Failed to run iptables")?;

    if !status.success() {
        return Err(anyhow::anyhow!("iptables DNAT rule failed"));
    }

    // SNAT rule for response traffic
    let status = Command::new("iptables")
        .args([
            "-t", "nat", "-A", "POSTROUTING",
            "-p", "tcp",
            "-d", container_ip,
            "--dport", &container_port.to_string(),
            "-j", "MASQUERADE",
        ])
        .status()
        .context("Failed to run iptables")?;

    if !status.success() {
        return Err(anyhow::anyhow!("iptables SNAT rule failed"));
    }

    tracing::info!("Set up iptables port forwarding: {} -> {}:{}", host_port, container_ip, container_port);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_forwarder_creation() {
        let pf = PortForwarder::new(8080, 80);
        assert_eq!(pf.host_port, 8080);
        assert_eq!(pf.container_port, 80);
        assert!(pf.process.is_none());
    }
}
