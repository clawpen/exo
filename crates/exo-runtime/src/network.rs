//! Network management for containers.
//!
//! This module handles port forwarding for rootless containers.
//! When containers run with isolated network namespaces, we need to
//! forward traffic from host ports to container ports.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// Default bridge name and subnet used for container networking.
pub const DEFAULT_BRIDGE_NAME: &str = "exo0";
pub const DEFAULT_BRIDGE_SUBNET: &str = "172.30.0.0/16";
pub const DEFAULT_BRIDGE_GATEWAY: &str = "172.30.0.1";

/// Network attachment details persisted for a container so it can be torn
/// down reliably after the daemon restarts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkState {
    pub mode: String,
    pub bridge: Option<String>,
    pub veth_host: Option<String>,
    pub veth_container: Option<String>,
    pub container_ip: Option<String>,
    pub gateway: Option<String>,
    pub subnet: Option<String>,
    pub nft_table: Option<String>,
}

impl NetworkState {
    pub fn is_empty(&self) -> bool {
        self.bridge.is_none()
            && self.veth_host.is_none()
            && self.veth_container.is_none()
            && self.container_ip.is_none()
            && self.nft_table.is_none()
    }
}

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

/// Full bridge setup for a container. Creates the bridge if needed, allocates
/// an IP from the default subnet, creates a veth pair, attaches the host side
/// to the bridge, moves the container side into the container's netns, and
/// configures it.
#[cfg(target_os = "linux")]
pub fn setup_bridge_for_container(
    state_dir: &Path,
    container_name: &str,
    container_pid: u32,
) -> Result<NetworkState> {
    let bridge = DEFAULT_BRIDGE_NAME.to_string();
    let subnet = DEFAULT_BRIDGE_SUBNET.to_string();
    let gateway = DEFAULT_BRIDGE_GATEWAY.to_string();

    ensure_bridge(&bridge, &subnet, &gateway)
        .with_context(|| format!("Failed to ensure bridge {}", bridge))?;

    let container_ip = allocate_ip(state_dir, container_name, &subnet)
        .with_context(|| format!("Failed to allocate IP for {}", container_name))?;

    let host_veth = format!("veth{}h", short_id(container_name));
    let container_veth = format!("veth{}c", short_id(container_name));

    // Clean up stale interfaces with the same names before creating new ones.
    let _ = delete_link(&host_veth);
    let _ = delete_link(&container_veth);

    create_veth_pair(&host_veth, &container_veth)
        .with_context(|| format!("Failed to create veth pair {}/{}", host_veth, container_veth))?;

    attach_veth_to_bridge(&host_veth, &bridge)
        .with_context(|| format!("Failed to attach {} to {}", host_veth, bridge))?;

    move_veth_to_namespace(&container_veth, container_pid)
        .with_context(|| format!("Failed to move {} to pid {}", container_veth, container_pid))?;

    setup_container_interface(
        &container_veth,
        &container_ip,
        &gateway,
        "16",
        container_pid,
    )
    .with_context(|| format!("Failed to configure container interface for {}", container_name))?;

    Ok(NetworkState {
        mode: "bridge".to_string(),
        bridge: Some(bridge),
        veth_host: Some(host_veth),
        veth_container: Some(container_veth),
        container_ip: Some(container_ip),
        gateway: Some(gateway),
        subnet: Some(subnet),
        nft_table: None,
    })
}

/// Tear down the bridge networking artifacts for a container.
#[cfg(target_os = "linux")]
pub fn teardown_bridge_network(
    state_dir: &Path,
    container_name: &str,
    state: &NetworkState,
) -> Result<()> {
    if let Some(veth) = &state.veth_host {
        let _ = delete_link(veth);
    }
    if let Some(veth) = &state.veth_container {
        let _ = delete_link(veth);
    }
    let _ = release_ip(state_dir, container_name);
    Ok(())
}

/// Create the bridge if it does not already exist and assign the gateway IP.
#[cfg(target_os = "linux")]
fn ensure_bridge(bridge: &str, subnet: &str, gateway: &str) -> Result<()> {
    // Check if bridge already exists.
    let exists = Command::new("ip")
        .args(["link", "show", "dev", bridge])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !exists {
        run_ip(["link", "add", bridge, "type", "bridge"])
            .with_context(|| format!("Failed to create bridge {}", bridge))?;
    }

    // Ensure it has the gateway IP and is up.
    let has_addr = Command::new("ip")
        .args(["addr", "show", "dev", bridge])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(gateway))
        .unwrap_or(false);

    if !has_addr {
        run_ip(["addr", "add", &format!("{}/{}", gateway, prefix_len(subnet)?), "dev", bridge])
            .with_context(|| format!("Failed to assign gateway IP to {}", bridge))?;
    }

    run_ip(["link", "set", bridge, "up"])
        .with_context(|| format!("Failed to bring bridge {} up", bridge))?;

    // Enable IP forwarding and NAT for the subnet so containers can reach the
    // outside world. Best-effort: ignore errors when not running as root.
    let _ = enable_ip_forwarding();
    let _ = ensure_nat_for_subnet(subnet);

    Ok(())
}

/// Allocate the next free IP in the subnet. Simple JSON lease file in state.
#[cfg(target_os = "linux")]
fn allocate_ip(state_dir: &Path, container_name: &str, subnet: &str) -> Result<String> {
    let lease_path = ipam_path(state_dir);
    let mut leases: IpamLeases = if lease_path.exists() {
        let content = std::fs::read_to_string(&lease_path)
            .with_context(|| format!("Failed to read IPAM leases from {:?}", lease_path))?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        IpamLeases::default()
    };

    // Remove any stale lease for this container first.
    leases.entries.retain(|e| e.name != container_name);

    let (network, prefix) = parse_cidr(subnet)?;
    let host_count = (1u32 << (32 - prefix)) - 3; // reserve network, gateway, broadcast
    let gateway_octets: u32 = DEFAULT_BRIDGE_GATEWAY.parse::<Ipv4Addr>()?.into();
    let network_octets: u32 = network.parse::<Ipv4Addr>()?.into();

    let mut candidate = network_octets + 2; // skip network + gateway
    while candidate != network_octets + (1u32 << (32 - prefix)) - 1 {
        if candidate == gateway_octets {
            candidate += 1;
            continue;
        }
        let ip = Ipv4Addr::from(candidate).to_string();
        if !leases.entries.iter().any(|e| e.ip == ip) {
            leases.entries.push(IpamLease {
                name: container_name.to_string(),
                ip: ip.clone(),
            });
            let json = serde_json::to_string_pretty(&leases)
                .context("Failed to serialize IPAM leases")?;
            std::fs::write(&lease_path, json)
                .with_context(|| format!("Failed to write IPAM leases to {:?}", lease_path))?;
            return Ok(ip);
        }
        candidate += 1;
        if candidate - network_octets > host_count {
            break;
        }
    }

    Err(anyhow::anyhow!("No free IP addresses in subnet {}", subnet))
}

/// Release an allocated IP.
#[cfg(target_os = "linux")]
fn release_ip(state_dir: &Path, container_name: &str) -> Result<()> {
    let lease_path = ipam_path(state_dir);
    if !lease_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&lease_path)
        .with_context(|| format!("Failed to read IPAM leases from {:?}", lease_path))?;
    let mut leases: IpamLeases = serde_json::from_str(&content).unwrap_or_default();
    leases.entries.retain(|e| e.name != container_name);
    let json = serde_json::to_string_pretty(&leases)
        .context("Failed to serialize IPAM leases")?;
    std::fs::write(&lease_path, json)
        .with_context(|| format!("Failed to write IPAM leases to {:?}", lease_path))?;
    Ok(())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IpamLeases {
    entries: Vec<IpamLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IpamLease {
    name: String,
    ip: String,
}

fn ipam_path(state_dir: &Path) -> PathBuf {
    state_dir.join("ipam.json")
}

/// Read the current IPAM lease table as a list of (name, ip) tuples suitable
/// for injection into `/etc/hosts`.
pub fn read_hosts_entries(state_dir: &Path) -> Vec<(String, String)> {
    let lease_path = ipam_path(state_dir);
    if !lease_path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(&lease_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let leases: IpamLeases = serde_json::from_str(&content).unwrap_or_default();
    leases
        .entries
        .into_iter()
        .map(|e| (e.name, e.ip))
        .collect()
}

/// Create a veth pair.
#[cfg(target_os = "linux")]
fn create_veth_pair(host_if: &str, container_if: &str) -> Result<()> {
    run_ip(["link", "add", host_if, "type", "veth", "peer", "name", container_if])
        .with_context(|| format!("Failed to create veth pair {}/{}", host_if, container_if))
}

/// Attach a veth host side to a bridge.
#[cfg(target_os = "linux")]
fn attach_veth_to_bridge(host_if: &str, bridge: &str) -> Result<()> {
    run_ip(["link", "set", host_if, "master", bridge, "up"])
        .with_context(|| format!("Failed to attach {} to {}", host_if, bridge))
}

/// Move the container-side veth into the container's network namespace.
#[cfg(target_os = "linux")]
fn move_veth_to_namespace(container_if: &str, pid: u32) -> Result<()> {
    run_ip(["link", "set", container_if, "netns", &pid.to_string()])
        .with_context(|| format!("Failed to move {} to netns of pid {}", container_if, pid))
}

/// Configure the container-side interface: rename, assign IP, bring up, default route.
#[cfg(target_os = "linux")]
fn setup_container_interface(
    old_name: &str,
    ip: &str,
    gateway: &str,
    prefix: &str,
    pid: u32,
) -> Result<()> {
    // Run inside the container's netns.
    let cidr = format!("{}/{}", ip, prefix);
    run_nsenter_net(pid, ["ip", "link", "set", old_name, "name", "eth0"])
        .with_context(|| format!("Failed to rename {} to eth0 in pid {}", old_name, pid))?;
    run_nsenter_net(pid, ["ip", "addr", "add", &cidr, "dev", "eth0"])
        .with_context(|| format!("Failed to add IP {} to eth0 in pid {}", cidr, pid))?;
    run_nsenter_net(pid, ["ip", "link", "set", "eth0", "up"])
        .with_context(|| format!("Failed to bring eth0 up in pid {}", pid))?;
    run_nsenter_net(pid, ["ip", "route", "add", "default", "via", gateway])
        .with_context(|| format!("Failed to add default route via {} in pid {}", gateway, pid))?;
    Ok(())
}

/// Delete a network interface, ignoring errors.
#[cfg(target_os = "linux")]
fn delete_link(ifname: &str) -> Result<()> {
    run_ip(["link", "delete", ifname])
        .with_context(|| format!("Failed to delete interface {}", ifname))
}

/// Run the `ip` command and return an error if it fails.
#[cfg(target_os = "linux")]
fn run_ip<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new("ip")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .context("Failed to execute ip command")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("ip command failed: {:?}", status))
    }
}

/// Run `nsenter --target <pid> --net -- ip ...`.
#[cfg(target_os = "linux")]
fn run_nsenter_net<I, S>(pid: u32, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let status = Command::new("nsenter")
        .arg("--target")
        .arg(pid.to_string())
        .arg("--net")
        .arg("--")
        .arg("ip")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .context("Failed to execute nsenter ip command")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("nsenter ip command failed: {:?}", status))
    }
}

/// Enable IPv4 forwarding. Best-effort; failures are logged but ignored.
#[cfg(target_os = "linux")]
fn enable_ip_forwarding() {
    let path = Path::new("/proc/sys/net/ipv4/ip_forward");
    if path.exists() {
        let _ = std::fs::write(path, b"1");
    }
}

/// Ensure nftables NAT masquerade exists for the bridge subnet.
#[cfg(target_os = "linux")]
fn ensure_nat_for_subnet(subnet: &str) -> Result<()> {
    // Prefer nftables; fall back to iptables.
    if is_command_available("nft") {
        let rule = format!(
            "table ip exo {{ chain postrouting {{ type nat hook postrouting priority 100; policy accept; ip saddr {} oif != {} masquerade }} }}",
            subnet, DEFAULT_BRIDGE_NAME
        );
        let status = Command::new("nft")
            .args(["-f", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                if let Some(stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(rule.as_bytes());
                }
                child.wait()
            });
        if status.map(|s| s.success()).unwrap_or(false) {
            return Ok(());
        }
    }

    if is_command_available("iptables") {
        let _ = Command::new("iptables")
            .args([
                "-t", "nat", "-A", "POSTROUTING",
                "-s", subnet,
                "!", "-o", DEFAULT_BRIDGE_NAME,
                "-j", "MASQUERADE",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    Ok(())
}

/// Parse a CIDR string into (network address, prefix length).
fn parse_cidr(cidr: &str) -> Result<(&str, u32)> {
    let (net, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("Invalid CIDR: {}", cidr))?;
    let prefix: u32 = prefix.parse().context("Invalid CIDR prefix")?;
    Ok((net, prefix))
}

fn prefix_len(subnet: &str) -> Result<u32> {
    parse_cidr(subnet).map(|(_, p)| p)
}

/// Create a short, deterministic identifier from a container name suitable for
/// use in interface names (max 11 chars to leave room for 'h'/'c' suffix).
fn short_id(name: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:x}", hash)[..11.min(16)].to_string()
}

/// Non-Linux stub for bridge setup.
#[cfg(not(target_os = "linux"))]
pub fn setup_bridge_for_container(
    _state_dir: &Path,
    _container_name: &str,
    _container_pid: u32,
) -> Result<NetworkState> {
    Ok(NetworkState::default())
}

/// Non-Linux stub for bridge teardown.
#[cfg(not(target_os = "linux"))]
pub fn teardown_bridge_network(
    _state_dir: &Path,
    _container_name: &str,
    _state: &NetworkState,
) -> Result<()> {
    Ok(())
}

/// nftables-based port forwarding for bridge-mode containers.
///
/// Each container gets its own `ip exo_<id>` table so teardown is a single
/// `nft delete table` operation.
#[cfg(target_os = "linux")]
pub struct NftablesPortForwarder {
    table_name: String,
}

#[cfg(target_os = "linux")]
impl NftablesPortForwarder {
    /// Create and populate the nftables rules for a container.
    pub fn new(
        container_name: &str,
        container_ip: &str,
        port_mappings: &[(u16, u16)],
    ) -> Result<Self> {
        let table_name = format!("exo_{}", short_id(container_name));

        // Build the ruleset in one go so it's atomic.
        let mut ruleset = format!(
            "table ip {} {{
  chain prerouting {{ type nat hook prerouting priority dstnat; policy accept;\n",
            table_name
        );
        for (host_port, container_port) in port_mappings {
            ruleset.push_str(&format!(
                "    tcp dport {} dnat to {}:{}\n",
                host_port, container_ip, container_port
            ));
        }
        ruleset.push_str(
            "  }\n  chain postrouting {{ type nat hook postrouting priority srcnat; policy accept;\n"
        );
        ruleset.push_str(&format!(
            "    ip daddr {} masquerade\n",
            container_ip
        ));
        ruleset.push_str("  }\n}\n");

        run_nft_ruleset(&ruleset)
            .with_context(|| format!("Failed to apply nftables ruleset for {}", container_name))?;

        tracing::info!(
            "Set up nftables port forwarding for {} (table {}): {:?}",
            container_name,
            table_name,
            port_mappings
        );

        Ok(Self { table_name })
    }

    /// Remove the container's nftables table.
    pub fn teardown(&self) -> Result<()> {
        let ruleset = format!("delete table ip {}\n", self.table_name);
        run_nft_ruleset(&ruleset)
            .with_context(|| format!("Failed to remove nftables table {}", self.table_name))?;
        tracing::info!("Removed nftables table {}", self.table_name);
        Ok(())
    }
}

/// Run a multi-line nftables ruleset via stdin.
#[cfg(target_os = "linux")]
fn run_nft_ruleset(ruleset: &str) -> Result<()> {
    let mut child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn nft")?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(ruleset.as_bytes())
            .context("Failed to write nft ruleset to stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for nft")?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!("nft failed: {}", stderr))
    }
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
