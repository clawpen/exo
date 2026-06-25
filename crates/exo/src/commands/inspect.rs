//! `exo inspect` — show detailed container metadata.

use anyhow::Result;
use std::path::PathBuf;

pub struct InspectArgs {
    pub container: String,
    pub json: bool,
}

pub async fn execute(args: InspectArgs) -> Result<()> {
    #[cfg(windows)]
    {
        return execute_windows(args).await;
    }

    #[cfg(not(windows))]
    {
        return execute_linux(args).await;
    }
}

#[cfg(windows)]
async fn execute_windows(_args: InspectArgs) -> Result<()> {
    anyhow::bail!("`exo inspect` is not yet supported on Windows; run inside WSL2.")
}

#[cfg(not(windows))]
async fn execute_linux(args: InspectArgs) -> Result<()> {
    use exo_runtime::{ContainerHandle, ContainerManager};

    let manager = ContainerManager::new()?;
    let metadata = manager
        .find(&args.container)?
        .ok_or_else(|| anyhow::anyhow!("Container not found: {}", args.container))?;

    let handle = ContainerHandle::new(metadata.name.clone(), metadata.config.clone());
    let upper_size = handle.upper_layer_size().unwrap_or(0);

    if args.json {
        let mut payload = serde_json::json!({
            "id": metadata.id,
            "name": metadata.name,
            "image": metadata.image,
            "status": metadata.status,
            "pid": metadata.pid,
            "created_at": metadata.created_at,
            "started_at": metadata.started_at,
            "stopped_at": metadata.stopped_at,
            "exit_code": metadata.exit_code,
            "restart_count": metadata.restart_count,
            "command": metadata.config.command,
            "workdir": metadata.config.workdir,
            "user": metadata.config.user,
            "hostname": metadata.config.hostname,
            "env": metadata.config.env,
            "resources": metadata.config.resources,
            "network": metadata.config.network,
            "mounts": metadata.config.mounts,
            "ports": metadata.ports,
            "labels": metadata.labels,
            "upper_layer_size": upper_size,
            "rootfs": handle.rootfs_path(),
            "upper_dir": handle.upper_path(),
        });

        // Add live cgroup stats if running.
        if metadata.is_running() {
            if let Some(pid) = metadata.pid {
                if let Ok(cgroup) = exo_runtime::CgroupManager::new(&metadata.name) {
                    let (io_rbytes, io_wbytes) = cgroup.get_io_stats().unwrap_or((0, 0));
                    let (cpu_periods, cpu_throttled, cpu_throttled_usec) =
                        cgroup.get_cpu_throttling().unwrap_or((0, 0, 0));
                    payload["stats"] = serde_json::json!({
                        "memory_usage": cgroup.get_memory_usage().ok(),
                        "memory_limit": cgroup.get_memory_limit().ok().flatten(),
                        "cpu_usage_ns": cgroup.get_cpu_usage().ok(),
                        "pids": cgroup.get_processes().unwrap_or_default().len() as u64,
                        "io_rbytes": io_rbytes,
                        "io_wbytes": io_wbytes,
                        "cpu_periods": cpu_periods,
                        "cpu_throttled": cpu_throttled,
                        "cpu_throttled_usec": cpu_throttled_usec,
                        "pid": pid,
                    });
                }
            }
        }

        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    let id_short = &metadata.id[..8.min(metadata.id.len())];
    println!("{:<20} {}", "Container:", metadata.name);
    println!("{:<20} {} ({})", "ID:", id_short, metadata.id);
    println!("{:<20} {}", "Image:", metadata.image);
    println!("{:<20} {}", "Status:", metadata.status);
    if let Some(pid) = metadata.pid {
        println!("{:<20} {}", "PID:", pid);
    }
    println!("{:<20} {}", "Created:", metadata.created_at);
    if let Some(started) = metadata.started_at {
        println!("{:<20} {}", "Started:", started);
    }
    if let Some(stopped) = metadata.stopped_at {
        println!("{:<20} {}", "Stopped:", stopped);
    }
    if let Some(code) = metadata.exit_code {
        println!("{:<20} {}", "Exit code:", code);
    }
    if metadata.restart_count > 0 {
        println!("{:<20} {}", "Restart count:", metadata.restart_count);
    }

    println!("\n{:<20} {}", "Command:", metadata.config.command.join(" "));
    println!("{:<20} {}", "Working dir:", metadata.config.workdir.display());
    println!("{:<20} {}", "User:", metadata.config.user);
    println!("{:<20} {}", "Hostname:", metadata.config.hostname);

    println!("\nEnvironment:");
    if metadata.config.env.is_empty() {
        println!("  (none)");
    } else {
        let mut keys: Vec<&String> = metadata.config.env.keys().collect();
        keys.sort();
        for k in keys {
            println!("  {}={}", k, metadata.config.env.get(k).unwrap_or(&String::new()));
        }
    }

    println!("\nResources:");
    if let Some(mem) = &metadata.config.resources.memory {
        println!("  Memory: {}", mem);
    }
    if let Some(cpu) = &metadata.config.resources.cpu {
        println!("  CPU: {}", cpu);
    }
    if let Some(pids) = metadata.config.resources.pids_limit {
        println!("  PIDs limit: {}", pids);
    }

    println!("\nNetwork:");
    println!("  Mode: {}", metadata.config.network.mode);
    if !metadata.config.network.port_mappings.is_empty() {
        println!("  Port mappings:");
        for pm in &metadata.config.network.port_mappings {
            println!(
                "    {}:{} -> {}/{}",
                pm.host_ip, pm.host_port, pm.container_port, pm.protocol
            );
        }
    }
    if !metadata.config.network.dns.is_empty() {
        println!("  DNS: {}", metadata.config.network.dns.join(", "));
    }

    if !metadata.config.mounts.is_empty() {
        println!("\nMounts:");
        for m in &metadata.config.mounts {
            let ro = if m.readonly { " (ro)" } else { "" };
            println!("  {} -> {}{}", m.source, m.target, ro);
        }
    }

    println!("\nStorage:");
    println!("  Rootfs: {}", handle.rootfs_path().display());
    println!("  Upper dir: {}", handle.upper_path().display());
    println!("  Writable layer: {}", human_bytes(upper_size));

    if metadata.is_running() {
        if let Ok(cgroup) = exo_runtime::CgroupManager::new(&metadata.name) {
            println!("\nLive stats:");
            if let Ok(mem) = cgroup.get_memory_usage() {
                println!("  Memory usage: {}", human_bytes(mem));
            }
            if let Ok(Some(limit)) = cgroup.get_memory_limit() {
                println!("  Memory limit: {}", human_bytes(limit));
            }
            if let Ok(cpu) = cgroup.get_cpu_usage() {
                println!("  CPU usage: {} ns", cpu);
            }
            let pids = cgroup.get_processes().unwrap_or_default().len();
            println!("  PIDs: {}", pids);
        }
    }

    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, UNITS[i])
}

/// Resolve a path spec that may be prefixed with `container:`.
/// Returns the owning container name (if any) and the path inside/outside.
pub fn parse_copy_spec(spec: &str) -> (Option<String>, PathBuf) {
    if let Some((container, path)) = spec.split_once(':') {
        (Some(container.to_string()), PathBuf::from(path))
    } else {
        (None, PathBuf::from(spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_copy_spec() {
        let (c, p) = parse_copy_spec("mycontainer:/etc/hostname");
        assert_eq!(c, Some("mycontainer".to_string()));
        assert_eq!(p, PathBuf::from("/etc/hostname"));

        let (c, p) = parse_copy_spec("/tmp/file.txt");
        assert_eq!(c, None);
        assert_eq!(p, PathBuf::from("/tmp/file.txt"));
    }

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0.0 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
    }
}
