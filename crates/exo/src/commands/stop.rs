//! Stop container command

use anyhow::Result;
#[cfg(all(not(windows), not(target_os = "macos")))]
use exo_runtime::ContainerManager;
#[cfg(windows)]
use exo_wsl::WslCommand;

pub struct StopArgs {
    pub container: String,
    pub force: bool,
    pub time: u64,
    pub backend: String,
    pub json: bool,
}

pub async fn execute(args: StopArgs) -> Result<()> {
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
async fn execute_windows(args: StopArgs) -> Result<()> {
    use exo_wsl::{WindowsPortForwarder, WslConfig};

    let wsl_cmd = WslCommand::new(WslConfig::default());

    // Check if container exists and is running
    let list_result = wsl_cmd.exec(&format!(
        "exo-runtime list 2>/dev/null | grep -w '{}' || echo 'NOT_FOUND'",
        args.container
    ))?;

    let list_output = list_result.stdout.trim();
    if list_output.contains("NOT_FOUND") || list_output.is_empty() {
        return Err(exo_runtime::ExoError::ContainerNotFound(args.container.clone()).into());
    }

    // Check if container is running
    if !list_output.contains("running") {
        emit_status(&args.container, "not_running", args.json);
        return Ok(());
    }

    // Stop the container
    if !args.json {
        if args.force {
            println!("Force stopping container: {}", args.container);
        } else {
            println!("Stopping container: {}", args.container);
        }
    }

    let stop_result = wsl_cmd.exec(&format!("exo-runtime stop {}", args.container))?;

    if stop_result.exit_code != 0 {
        anyhow::bail!("Failed to stop container: {}", stop_result.stderr);
    }

    // Remove port forwarding rules
    let forwarder = WindowsPortForwarder::new(WslConfig::default());
    let _ = forwarder.remove_port_forward(&args.container);

    emit_status(&args.container, "stopped", args.json);
    Ok(())
}

#[cfg(target_os = "macos")]
async fn execute_macos(args: StopArgs) -> Result<()> {
    use exo_runtime::{ExoBackend, StopOptions};

    match super::mac::select_backend(&args.backend)? {
        super::mac::BackendSelection::Native => {
            let output =
                super::mac::native_backend()?.stop(&args.container, args.force, args.time)?;
            if args.json {
                emit_status(&args.container, "stopped", true);
            } else {
                print!("{}", output);
            }
        }
        super::mac::BackendSelection::Linux => {
            super::mac::linux_backend()
                .stop(
                    &args.container,
                    StopOptions {
                        force: args.force,
                        timeout_secs: args.time,
                    },
                )
                .await?;
            if args.json {
                emit_status(&args.container, "stopped", true);
            } else {
                println!("Container {} stopped in the EXO Linux VM", args.container);
            }
        }
    }
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
async fn execute_linux(args: StopArgs) -> Result<()> {
    let manager = ContainerManager::new()?;

    // Find container by name or ID
    let mut metadata = manager
        .find(&args.container)?
        .ok_or_else(|| exo_runtime::ExoError::ContainerNotFound(args.container.clone()))?;

    // Check if container is running
    if !metadata.is_running() {
        emit_status(&metadata.name, "not_running", args.json);
        return Ok(());
    }

    let pid = metadata
        .pid
        .ok_or_else(|| anyhow::anyhow!("Container has no PID"))?;

    // Send signal to stop the container
    if args.force {
        if !args.json {
            println!("Force stopping container: {}", metadata.name);
        }
        // Send SIGKILL
        send_signal(pid, 9)?;
    } else {
        if !args.json {
            println!(
                "Stopping container: {} (waiting {}s)",
                metadata.name, args.time
            );
        }
        // Send SIGTERM
        send_signal(pid, 15)?;

        // Wait for process to exit
        let waited = wait_for_exit(pid, args.time);
        if !waited {
            if !args.json {
                println!("Container did not stop gracefully, sending SIGKILL");
            }
            send_signal(pid, 9)?;
            // Give it a moment to die
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    // Update metadata
    metadata.set_stopped(None);
    manager.save(&metadata)?;

    emit_status(&metadata.name, "stopped", args.json);

    Ok(())
}

/// Emit the lifecycle outcome via the shared contract helper.
fn emit_status(container: &str, status: &str, json: bool) {
    super::emit_lifecycle_status(container, status, json)
}

/// Send a signal to a process.
fn send_signal(pid: u32, signal: i32) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;

        let signal = match signal {
            9 => Signal::SIGKILL,
            15 => Signal::SIGTERM,
            _ => return Err(anyhow::anyhow!("Unsupported signal: {}", signal)),
        };

        kill(Pid::from_raw(pid as i32), signal)
            .map_err(|e| anyhow::anyhow!("Failed to send signal: {}", e))?;
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Fallback for non-Linux systems
        let output = std::process::Command::new("kill")
            .arg(format!("-{}", signal))
            .arg(pid.to_string())
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to send signal: {}", e))?;

        if !output.status.success() {
            return Err(anyhow::anyhow!("Failed to send signal to process {}", pid));
        }
    }

    Ok(())
}

/// Wait for a process to exit.
fn wait_for_exit(pid: u32, timeout_secs: u64) -> bool {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    loop {
        // Check if process still exists
        let proc_path = format!("/proc/{}", pid);
        if !std::path::Path::new(&proc_path).exists() {
            return true;
        }

        if start.elapsed() >= timeout {
            return false;
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
