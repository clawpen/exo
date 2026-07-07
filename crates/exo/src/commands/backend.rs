//! Backend inspection commands.

use exo_runtime::BackendCapabilities;
use serde::Serialize;

pub struct BackendInfoArgs {
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct BackendInfo {
    active: &'static str,
    selection_default: &'static str,
    capabilities: BackendCapabilities,
    notes: Vec<&'static str>,
}

pub async fn info(args: BackendInfoArgs) -> anyhow::Result<()> {
    let info = current_backend_info();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    println!("Active backend: {}", info.active);
    println!("Default selection: {}", info.selection_default);
    println!("Capabilities:");
    print_capability("linux_containers", info.capabilities.linux_containers);
    print_capability("native_processes", info.capabilities.native_processes);
    print_capability("gpu", info.capabilities.gpu);
    print_capability("metal", info.capabilities.metal);
    print_capability("cgroups", info.capabilities.cgroups);
    print_capability("namespaces", info.capabilities.namespaces);
    print_capability("seccomp", info.capabilities.seccomp);
    print_capability("overlayfs", info.capabilities.overlayfs);
    print_capability("port_forwarding", info.capabilities.port_forwarding);
    print_capability("volume_mounts", info.capabilities.volume_mounts);
    print_capability("daemon", info.capabilities.daemon);
    print_capability("rootless", info.capabilities.rootless);

    if !info.notes.is_empty() {
        println!("Notes:");
        for note in info.notes {
            println!("  - {}", note);
        }
    }

    Ok(())
}

fn print_capability(name: &str, value: bool) {
    println!("  {:<18} {}", name, if value { "yes" } else { "no" });
}

#[cfg(target_os = "macos")]
fn current_backend_info() -> BackendInfo {
    BackendInfo {
        active: "native-macos",
        selection_default: "auto",
        capabilities: BackendCapabilities::native_macos(),
        notes: vec![
            "Native macOS mode runs host processes with env isolation and optional sandbox-exec.",
            "Linux OCI container mode on macOS requires the future Exo-managed microVM backend.",
        ],
    }
}

#[cfg(windows)]
fn current_backend_info() -> BackendInfo {
    BackendInfo {
        active: "windows-wsl2",
        selection_default: "auto",
        capabilities: BackendCapabilities::windows_wsl2(),
        notes: vec!["Windows uses the WSL2 Linux backend."],
    }
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn current_backend_info() -> BackendInfo {
    BackendInfo {
        active: "linux-runtime",
        selection_default: "auto",
        capabilities: BackendCapabilities::linux_runtime(),
        notes: vec!["Linux host runtime provides Linux container semantics directly."],
    }
}
