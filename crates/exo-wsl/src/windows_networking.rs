//! Windows networking support for WSL2 port forwarding.
//!
//! WSL2 uses a virtual network with dynamic IPs. This module provides
//! port forwarding from Windows to WSL2 containers using netsh.

use crate::{WslCommand, WslConfig};
use crate::networking::PortProtocol;
use anyhow::Result;
use std::collections::HashMap;
use tracing::{info, warn, debug};

/// Port forwarding rule on Windows
#[derive(Debug, Clone)]
pub struct PortForwardingRule {
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: PortProtocol,
    pub listen_address: String,
}

/// Manages Windows port forwarding to WSL2 containers
pub struct WindowsPortForwarder {
    wsl_command: WslCommand,
    wsl_ip_cache: std::sync::Mutex<Option<String>>,
    active_rules: std::sync::Mutex<HashMap<String, PortForwardingRule>>,
}

impl WindowsPortForwarder {
    /// Create a new port forwarder
    pub fn new(config: WslConfig) -> Self {
        Self {
            wsl_command: WslCommand::new(config),
            wsl_ip_cache: std::sync::Mutex::new(None),
            active_rules: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Get the WSL2 VM IP address
    fn get_wsl_ip(&self) -> Result<String> {
        // Check cache first
        if let Some(ip) = self.wsl_ip_cache.lock().unwrap().as_ref() {
            return Ok(ip.clone());
        }

        // Get WSL IP via /etc/resolv.conf (use cut instead of awk for better shell compatibility)
        let result = self.wsl_command.exec("grep nameserver /etc/resolv.conf | cut -d' ' -f2")?;

        if result.exit_code == 0 {
            let ip = result.stdout.trim().to_string();
            if !ip.is_empty() {
                *self.wsl_ip_cache.lock().unwrap() = Some(ip.clone());
                return Ok(ip);
            }
        }

        // Fallback: try to get IP from hostname -I
        let result = self.wsl_command.exec("hostname -I | awk '{print $1}'")?;
        if result.exit_code == 0 {
            let ip = result.stdout.trim().to_string();
            if !ip.is_empty() {
                *self.wsl_ip_cache.lock().unwrap() = Some(ip.clone());
                return Ok(ip);
            }
        }

        Err(anyhow::anyhow!("Could not determine WSL2 IP address"))
    }

    /// Add a port forwarding rule
    pub fn add_port_forward(&self, container_name: &str, rule: PortForwardingRule) -> Result<()> {
        let wsl_ip = self.get_wsl_ip()?;

        info!(
            "Setting up port forwarding: Windows:{} -> WSL@{}:{}",
            rule.host_port, wsl_ip, rule.container_port
        );

        // Check if we have admin privileges first
        let test_output = std::process::Command::new("netsh")
            .args(["interface", "portproxy", "show", "all"])
            .output();

        if let Ok(output) = test_output {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("elevation") || stderr.contains("access is denied") || !output.status.success() {
                warn!("Port forwarding requires administrator privileges. Run exo as administrator to use --publish.");
                anyhow::bail!("Port forwarding requires administrator privileges. Please run exo as administrator.");
            }
        }

        // Remove existing rule if present
        self.remove_port_forward(container_name)?;

        // Add netsh portproxy rule
        let netsh_cmd = format!(
            "netsh interface portproxy add {} listenport={} listenaddress={} connectport={} connectaddress={}",
            rule.protocol.as_str(),
            rule.host_port,
            rule.listen_address,
            rule.container_port,
            wsl_ip
        );

        debug!("Executing netsh command: {}", netsh_cmd);

        let output = std::process::Command::new("cmd")
            .args(["/C", &netsh_cmd])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Check for common errors
            if stderr.contains("elevation") || stderr.contains("access is denied") {
                anyhow::bail!("Port forwarding requires administrator privileges. Please run exo as administrator.");
            }
            // Netsh might fail if rule already exists - try to remove and add again
            if stderr.contains("already exists") || stderr.contains("access is denied") {
                warn!("Port forwarding rule may already exist, attempting to recreate");
                self.remove_port_forward(container_name)?;
                let output = std::process::Command::new("cmd")
                    .args(["/C", &netsh_cmd])
                    .output()?;
                if !output.status.success() {
                    anyhow::bail!("Failed to add port forwarding: {}", stderr);
                }
            } else {
                anyhow::bail!("Failed to add port forwarding: {}", stderr);
            }
        }

        // Store the active rule
        let key = format!("{}_{}_{}", container_name, rule.protocol.as_str(), rule.host_port);
        self.active_rules.lock().unwrap().insert(key, rule.clone());

        info!("Port forwarding established: {}:{} -> {}", rule.host_port, rule.container_port, container_name);

        // Also add Windows Firewall rule
        self.add_firewall_rule(&rule)?;

        Ok(())
    }

    /// Remove a port forwarding rule
    pub fn remove_port_forward(&self, container_name: &str) -> Result<()> {
        let mut rules = self.active_rules.lock().unwrap();
        let prefix = format!("{}_", container_name);
        let keys_to_remove: Vec<_> = rules
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();

        for key in keys_to_remove {
            if let Some(rule) = rules.remove(&key) {
                let netsh_cmd = format!(
                    "netsh interface portproxy delete {} listenport={} listenaddress={}",
                    rule.protocol.as_str(),
                    rule.host_port,
                    rule.listen_address
                );

                debug!("Executing netsh delete command: {}", netsh_cmd);

                let _ = std::process::Command::new("cmd")
                    .args(["/C", &netsh_cmd])
                    .output();

                info!("Removed port forwarding: {}:{} (for {})", rule.host_port, rule.container_port, container_name);
            }
        }

        Ok(())
    }

    /// Add Windows Firewall rule for the port
    fn add_firewall_rule(&self, rule: &PortForwardingRule) -> Result<()> {
        let rule_name = format!("Exo-Port-{}", rule.host_port);
        let firewall_cmd = format!(
            "netsh advfirewall firewall add rule name=\"{}\" dir=in action=allow protocol={} localport={}",
            rule_name,
            rule.protocol.as_str(),
            rule.host_port
        );

        debug!("Executing firewall command: {}", firewall_cmd);

        let output = std::process::Command::new("cmd")
            .args(["/C", &firewall_cmd])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Don't fail if rule already exists
            if !stderr.contains("already exists") {
                warn!("Could not add firewall rule: {}", stderr);
            }
        }

        Ok(())
    }

    /// List all active port forwarding rules
    pub fn list_rules(&self) -> Result<Vec<PortForwardingRule>> {
        let output = std::process::Command::new("cmd")
            .args(["/C", "netsh interface portproxy show all"])
            .output()?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut rules = Vec::new();

        for line in stdout.lines() {
            if line.contains("0.0.0.0") || line.contains("127.0.0.1") {
                // Parse netsh output format:
                // Listen on: IPv4: 0.0.0.0:8080
                // Connect to: IPv4: 172.x.x.x:8080
                let parts: Vec<_> = line.split_whitespace().collect();
                if parts.len() >= 6 {
                    if let (Some(listen_part), Some(connect_part)) = (parts.get(3), parts.get(6)) {
                        if let (Some(listen_port), Some(connect_port)) = (
                            listen_part.split(':').last(),
                            connect_part.split(':').last()
                        ) {
                            if let (Ok(lp), Ok(cp)) = (listen_port.parse::<u16>(), connect_port.parse::<u16>()) {
                                rules.push(PortForwardingRule {
                                    host_port: lp,
                                    container_port: cp,
                                    protocol: PortProtocol::Tcp, // Default to TCP
                                    listen_address: "0.0.0.0".to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(rules)
    }

    /// Clear all port forwarding rules
    pub fn clear_all_rules(&self) -> Result<()> {
        let rules = self.list_rules()?;
        for rule in rules {
            let netsh_cmd = format!(
                "netsh interface portproxy delete {} listenport={} listenaddress={}",
                rule.protocol.as_str(),
                rule.host_port,
                rule.listen_address
            );

            let _ = std::process::Command::new("cmd")
                .args(["/C", &netsh_cmd])
                .output();
        }

        self.active_rules.lock().unwrap().clear();
        Ok(())
    }

    /// Refresh WSL IP cache (call when WSL restarts)
    pub fn refresh_wsl_ip(&self) -> Result<()> {
        *self.wsl_ip_cache.lock().unwrap() = None;
        self.get_wsl_ip()?;
        Ok(())
    }
}
