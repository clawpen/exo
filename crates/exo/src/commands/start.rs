//! Start container command - restart a stopped container

use anyhow::Result;
#[cfg(all(not(windows), not(target_os = "macos")))]
use exo_runtime::ContainerManager;
#[cfg(all(not(windows), not(target_os = "macos")))]
use exo_runtime::{Container, ContainerStatus};
#[cfg(windows)]
use exo_wsl::WslCommand;

pub struct StartArgs {
    pub container: String,
    /// Attach to container (follow logs)
    pub attach: bool,
    pub backend: String,
    pub json: bool,
}

pub async fn execute(args: StartArgs) -> Result<()> {
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
async fn execute_windows(args: StartArgs) -> Result<()> {
    use exo_wsl::WslConfig;

    let wsl_cmd = WslCommand::new(WslConfig::default());

    // Check if container exists
    let list_result = wsl_cmd.exec(&format!(
        "exo-runtime list --all 2>/dev/null | grep -w '{}' || echo 'NOT_FOUND'",
        args.container
    ))?;

    let list_output = list_result.stdout.trim();
    if list_output.contains("NOT_FOUND") || list_output.is_empty() {
        return Err(exo_runtime::ExoError::ContainerNotFound(args.container.clone()).into());
    }

    // Check if container is already running
    if list_output.contains("running") {
        super::emit_lifecycle_status(&args.container, "already_running", args.json);
        return Ok(());
    }

    if !args.json {
        println!("Starting container: {}", args.container);
    }

    // Start the container
    let start_result = wsl_cmd.exec(&format!("exo-runtime start {}", args.container))?;

    if start_result.exit_code != 0 {
        anyhow::bail!("Failed to start container: {}", start_result.stderr);
    }

    super::emit_lifecycle_status(&args.container, "started", args.json);

    if args.attach {
        // Follow logs
        let logs_result = wsl_cmd.exec(&format!("exo-runtime logs -f {}", args.container))?;
        print!("{}", logs_result.stdout);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
async fn execute_macos(args: StartArgs) -> Result<()> {
    use exo_runtime::{ExoBackend, StartOptions};

    match super::mac::select_backend(&args.backend)? {
        super::mac::BackendSelection::Native => {
            let output = super::mac::native_backend()?.start(&args.container, args.attach)?;
            if args.json {
                super::emit_lifecycle_status(&args.container, "started", true);
            } else {
                print!("{}", output);
            }
        }
        super::mac::BackendSelection::Linux => {
            super::mac::linux_backend()
                .start(
                    &args.container,
                    StartOptions {
                        attach: args.attach,
                    },
                )
                .await?;
            if args.json {
                super::emit_lifecycle_status(&args.container, "started", true);
            } else {
                println!("Container {} started in the EXO Linux VM", args.container);
            }
        }
    }
    Ok(())
}

#[cfg(all(not(windows), not(target_os = "macos")))]
async fn execute_linux(args: StartArgs) -> Result<()> {
    let manager = ContainerManager::new()?;

    // Find container by name or ID
    let mut metadata = manager
        .find(&args.container)?
        .ok_or_else(|| exo_runtime::ExoError::ContainerNotFound(args.container.clone()))?;

    // Check if container is already running
    if metadata.is_running() {
        super::emit_lifecycle_status(&metadata.name, "already_running", args.json);
        return Ok(());
    }

    // Refresh status to make sure it's actually stopped
    manager.refresh_status(&mut metadata)?;

    if metadata.is_running() {
        super::emit_lifecycle_status(&metadata.name, "already_running", args.json);
        return Ok(());
    }

    if !args.json {
        println!("Starting container: {}", metadata.name);
    }

    // Create a new container from the saved config
    let config = metadata.config.clone();
    let mut container = Container::new(config)?;

    // Start the container
    container.start()?;

    // Update metadata with new state
    let pid = container.handle().pid.unwrap();
    metadata.id = container.handle().id.clone();
    metadata.set_running(pid);

    // Save updated metadata
    manager.save(&metadata)?;

    if args.json {
        super::emit_lifecycle_status(&metadata.name, "started", true);
    } else {
        println!("Container {} started (PID: {})", metadata.name, pid);
    }

    if args.attach {
        // Wait for container to finish
        let status = container.wait()?;

        // Update metadata with exit status
        if let ContainerStatus::Exited(code) = status {
            metadata.set_stopped(Some(code));
        } else {
            metadata.set_stopped(None);
        }

        manager.save(&metadata)?;

        match status {
            ContainerStatus::Exited(code) => {
                if code != 0 {
                    return Err(anyhow::anyhow!("Container exited with code {}", code));
                }
            }
            _ => {}
        }
    }

    Ok(())
}
