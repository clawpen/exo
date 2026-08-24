//! Remove container command

use anyhow::Result;
#[cfg(all(not(windows), not(target_os = "macos")))]
use exo_runtime::ContainerManager;
#[cfg(windows)]
use exo_wsl::WslCommand;

pub struct RemoveArgs {
    pub container: String,
    pub force: bool,
    pub backend: String,
    pub json: bool,
}

pub async fn execute(args: RemoveArgs) -> Result<()> {
    #[cfg(windows)]
    {
        return execute_windows(args).await;
    }

    #[cfg(target_os = "macos")]
    {
        return execute_macos(args).await;
    }

    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        return execute_linux(args).await;
    }
}

#[cfg(windows)]
async fn execute_windows(args: RemoveArgs) -> Result<()> {
    use exo_wsl::{WindowsPortForwarder, WslConfig};

    let wsl_cmd = WslCommand::new(WslConfig::default());

    // Check if container exists and is running
    let list_result = wsl_cmd.exec(&format!(
        "exo-runtime list --all 2>/dev/null | grep -w '{}' || echo 'NOT_FOUND'",
        args.container
    ))?;

    let list_output = list_result.stdout.trim();
    if list_output.contains("NOT_FOUND") || list_output.is_empty() {
        return Err(exo_runtime::ExoError::ContainerNotFound(args.container.clone()).into());
    }

    // Check if container is running
    if list_output.contains("running") && !args.force {
        return Err(exo_runtime::ExoError::ContainerRunning(format!(
            "{} (use --force to stop and remove)",
            args.container
        ))
        .into());
    }

    // If running and force, stop first
    if list_output.contains("running") && args.force {
        if !args.json {
            println!("Stopping container {} before removal", args.container);
        }
        let _ = wsl_cmd.exec(&format!("exo-runtime stop {}", args.container));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    // Remove the container
    // First try to unmount any overlay mounts that might still be active
    let _ = wsl_cmd.exec(&format!(
        "umount /var/lib/exo/containers/{}/rootfs 2>/dev/null || true",
        args.container
    ));
    std::thread::sleep(std::time::Duration::from_millis(200));

    let remove_result = wsl_cmd.exec(&format!("exo-runtime rm {}", args.container))?;

    if remove_result.exit_code != 0 {
        // If removal still fails, try force remove
        let force_result = wsl_cmd.exec(&format!(
            "rm -rf /var/lib/exo/containers/{} 2>/dev/null && echo 'REMOVED' || echo 'FAILED'",
            args.container
        ))?;
        if force_result.stdout.contains("REMOVED") {
            super::emit_lifecycle_status(&args.container, "removed", args.json);
            return Ok(());
        }
        anyhow::bail!("Failed to remove container: {}", remove_result.stderr);
    }

    // Remove port forwarding rules
    let forwarder = WindowsPortForwarder::new(WslConfig::default());
    let _ = forwarder.remove_port_forward(&args.container);

    super::emit_lifecycle_status(&args.container, "removed", args.json);
    Ok(())
}

#[cfg(target_os = "macos")]
async fn execute_macos(args: RemoveArgs) -> Result<()> {
    use exo_runtime::{ExoBackend, RemoveOptions};

    match super::mac::select_backend(&args.backend)? {
        super::mac::BackendSelection::Native => {
            let output = super::mac::native_backend()?.remove(&args.container, args.force)?;
            if args.json {
                super::emit_lifecycle_status(&args.container, "removed", true);
            } else {
                print!("{}", output);
            }
        }
        super::mac::BackendSelection::Linux => {
            super::mac::linux_backend()
                .remove(&args.container, RemoveOptions { force: args.force })
                .await?;
            if args.json {
                super::emit_lifecycle_status(&args.container, "removed", true);
            } else {
                println!("Container {} removed from the EXO Linux VM", args.container);
            }
        }
    }
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
async fn execute_linux(args: RemoveArgs) -> Result<()> {
    let manager = ContainerManager::new()?;

    // Find container by name or ID
    let metadata = manager
        .find(&args.container)?
        .ok_or_else(|| exo_runtime::ExoError::ContainerNotFound(args.container.clone()))?;

    // Check if container is running
    if metadata.is_running() && !args.force {
        return Err(exo_runtime::ExoError::ContainerRunning(format!(
            "{} (use --force to stop and remove)",
            metadata.name
        ))
        .into());
    }

    // If running and force, stop first
    if metadata.is_running() && args.force {
        if !args.json {
            println!("Stopping container {} before removal", metadata.name);
        }

        if let Some(pid) = metadata.pid {
            // Send SIGKILL
            #[cfg(target_os = "linux")]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;

                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
            }

            #[cfg(not(target_os = "linux"))]
            {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .output();
            }

            // Wait a moment for process to die
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    // Remove container metadata
    manager.remove(&metadata.name)?;

    super::emit_lifecycle_status(&metadata.name, "removed", args.json);

    Ok(())
}
