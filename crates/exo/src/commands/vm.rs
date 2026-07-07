//! `exo vm` command implementation.

pub async fn init(force: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let manager = exo_vm_mac::VmManager::new(exo_vm_mac::VmConfig::default())?;
        manager.init(force).await?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = force;
        anyhow::bail!("'exo vm' is only supported on macOS")
    }
}

pub async fn start(foreground: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if foreground {
            let mut manager = exo_vm_mac::VmManager::new(exo_vm_mac::VmConfig::default())?;
            manager.start(true)?;
            return Ok(());
        }

        start_daemon()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = foreground;
        anyhow::bail!("'exo vm' is only supported on macOS")
    }
}

pub async fn stop(force: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let client = exo_vm_mac::VmDaemonClient::new()?;
        if client.is_running() {
            match client.stop(force)? {
                exo_vm_mac::VmDaemonResponse::Stopped => {
                    println!("VM stopped");
                    return Ok(());
                }
                exo_vm_mac::VmDaemonResponse::Error { message } => anyhow::bail!("{}", message),
                other => anyhow::bail!("unexpected VM daemon response: {:?}", other),
            }
        }

        println!("VM daemon is not running");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = force;
        anyhow::bail!("'exo vm' is only supported on macOS")
    }
}

pub async fn status(json: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let client = exo_vm_mac::VmDaemonClient::new()?;
        if client.is_running() {
            match client.status()? {
                exo_vm_mac::VmDaemonResponse::Status {
                    running,
                    guest_agent_reachable,
                    guest_agent_info,
                } => {
                    if json {
                        let obj = serde_json::json!({
                            "daemon": "running",
                            "socket": client.socket_path(),
                            "status": if running { "running" } else { "stopped" },
                            "guest_agent_reachable": guest_agent_reachable,
                            "guest_agent_info": guest_agent_info,
                        });
                        println!("{}", serde_json::to_string_pretty(&obj)?);
                    } else {
                        println!("VM daemon: running");
                        println!("Socket: {}", client.socket_path().display());
                        println!("Status: {}", if running { "running" } else { "stopped" });
                        println!(
                            "Guest agent: {}",
                            if guest_agent_reachable {
                                "reachable"
                            } else {
                                "unreachable"
                            }
                        );
                        if !guest_agent_info.is_empty() {
                            println!("Guest info: {}", guest_agent_info);
                        }
                    }
                    return Ok(());
                }
                exo_vm_mac::VmDaemonResponse::Error { message } => anyhow::bail!("{}", message),
                other => anyhow::bail!("unexpected VM daemon response: {:?}", other),
            }
        }

        if json {
            let obj = serde_json::json!({
                "daemon": "stopped",
                "socket": client.socket_path(),
                "status": "stopped",
                "guest_agent_reachable": false,
                "guest_agent_info": "",
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("VM daemon: stopped");
            println!("Socket: {}", client.socket_path().display());
            println!("Status: stopped");
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = json;
        anyhow::bail!("'exo vm' is only supported on macOS")
    }
}

pub async fn serve() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        exo_vm_mac::daemon::serve_foreground(exo_vm_mac::VmConfig::default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("'exo vm serve' is only supported on macOS")
    }
}

pub async fn install_guest_agent(path: std::path::PathBuf) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if !path.exists() {
            anyhow::bail!("guest agent binary not found: {}", path.display());
        }
        let dest = exo_vm_mac::guest_agent_binary_path()?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&path, &dest)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&dest)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dest, perms)?;
        }
        println!("Installed guest agent: {}", dest.display());
        println!("Run 'exo vm init --force' to rebuild the initrd with this agent.");
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        anyhow::bail!("'exo vm install-guest-agent' is only supported on macOS")
    }
}

pub async fn import_image(image: String, guest_path: String) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let backend = exo_vm_mac::MacLinuxBackend::new(exo_vm_mac::VmConfig::default());
        let rootfs = backend.import_image_from_guest_path(&image, &guest_path)?;
        println!("Imported image {} to {}", image, rootfs);
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, guest_path);
        anyhow::bail!("'exo vm import-image' is only supported on macOS")
    }
}

#[cfg(target_os = "macos")]
fn start_daemon() -> anyhow::Result<()> {
    use std::fs::OpenOptions;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let client = exo_vm_mac::VmDaemonClient::new()?;
    if client.is_running() {
        println!("VM daemon already running");
        println!("Socket: {}", client.socket_path().display());
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let log_path = exo_vm_mac::daemon_log_path()?;
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let child = Command::new(exe)
        .args(["vm", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    println!("Starting VM daemon (PID: {})", child.id());
    println!("Log: {}", log_path.display());

    // Detach from the child. The daemon keeps the VM handle alive.
    std::mem::forget(child);

    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if client.is_running() {
            println!("VM daemon running");
            println!("Socket: {}", client.socket_path().display());
            return Ok(());
        }
    }

    anyhow::bail!(
        "timed out waiting for VM daemon to become ready; see {}",
        log_path.display()
    )
}

pub async fn reset(keep_state: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut manager = exo_vm_mac::VmManager::new(exo_vm_mac::VmConfig::default())?;
        manager.reset(keep_state).await?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = keep_state;
        anyhow::bail!("'exo vm' is only supported on macOS")
    }
}
