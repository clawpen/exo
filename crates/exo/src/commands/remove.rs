//! Remove container command

#[cfg(windows)]
use exo_wsl::WslCommand;
use exo_runtime::ContainerManager;
use anyhow::Result;

pub struct RemoveArgs {
    pub container: String,
    pub force: bool,
}

pub async fn execute(args: RemoveArgs) -> Result<()> {
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
async fn execute_windows(args: RemoveArgs) -> Result<()> {
    use exo_wsl::{WslConfig, WindowsPortForwarder};

    let wsl_cmd = WslCommand::new(WslConfig::default());

    // Check if container exists and is running
    let list_result = wsl_cmd.exec(&format!(
        "exo-runtime list --all 2>/dev/null | grep -w '{}' || echo 'NOT_FOUND'",
        args.container
    ))?;

    let list_output = list_result.stdout.trim();
    if list_output.contains("NOT_FOUND") || list_output.is_empty() {
        anyhow::bail!("Container not found: {}", args.container);
    }

    // Check if container is running
    if list_output.contains("running") && !args.force {
        anyhow::bail!("Container {} is running. Use --force to stop and remove.", args.container);
    }

    // If running and force, stop first
    if list_output.contains("running") && args.force {
        println!("Stopping container {} before removal", args.container);
        let _ = wsl_cmd.exec(&format!("exo-runtime stop {}", args.container));
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    // Remove the container
    // First try to unmount any overlay mounts that might still be active
    let _ = wsl_cmd.exec(&format!("umount /var/lib/exo/containers/{}/rootfs 2>/dev/null || true", args.container));
    std::thread::sleep(std::time::Duration::from_millis(200));

    let remove_result = wsl_cmd.exec(&format!("exo-runtime rm {}", args.container))?;

    if remove_result.exit_code != 0 {
        // If removal still fails, try force remove
        let force_result = wsl_cmd.exec(&format!("rm -rf /var/lib/exo/containers/{} 2>/dev/null && echo 'REMOVED' || echo 'FAILED'", args.container))?;
        if force_result.stdout.contains("REMOVED") {
            println!("Container {} removed", args.container);
            return Ok(());
        }
        anyhow::bail!("Failed to remove container: {}", remove_result.stderr);
    }

    // Remove port forwarding rules
    let forwarder = WindowsPortForwarder::new(WslConfig::default());
    let _ = forwarder.remove_port_forward(&args.container);

    println!("Container {} removed", args.container);
    Ok(())
}

#[cfg(not(windows))]
async fn execute_linux(args: RemoveArgs) -> Result<()> {
    let manager = ContainerManager::new()?;

    // Find container by name or ID
    let metadata = manager.find(&args.container)?
        .ok_or_else(|| anyhow::anyhow!("Container not found: {}", args.container))?;
    
    // Check if container is running
    if metadata.is_running() && !args.force {
        anyhow::bail!("Container {} is running. Use --force to stop and remove.", metadata.name);
    }
    
    // If running and force, stop first
    if metadata.is_running() && args.force {
        println!("Stopping container {} before removal", metadata.name);
        
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
    
    println!("Container {} removed", metadata.name);
    
    Ok(())
}
