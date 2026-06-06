//! Start container command - restart a stopped container

#[cfg(windows)]
use exo_wsl::WslCommand;
use exo_runtime::ContainerManager;
use exo_runtime::{Container, ContainerStatus};
use anyhow::Result;

pub struct StartArgs {
    pub container: String,
    /// Attach to container (follow logs)
    pub attach: bool,
}

pub async fn execute(args: StartArgs) -> Result<()> {
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
        anyhow::bail!("Container not found: {}", args.container);
    }

    // Check if container is already running
    if list_output.contains("running") {
        println!("Container {} is already running", args.container);
        return Ok(());
    }

    println!("Starting container: {}", args.container);

    // Start the container
    let start_result = wsl_cmd.exec(&format!("exo-runtime start {}", args.container))?;

    if start_result.exit_code != 0 {
        anyhow::bail!("Failed to start container: {}", start_result.stderr);
    }

    println!("Container {} started", args.container);

    if args.attach {
        // Follow logs
        let logs_result = wsl_cmd.exec(&format!("exo-runtime logs -f {}", args.container))?;
        print!("{}", logs_result.stdout);
    }

    Ok(())
}

#[cfg(not(windows))]
async fn execute_linux(args: StartArgs) -> Result<()> {
    let manager = ContainerManager::new()?;
    
    // Find container by name or ID
    let mut metadata = manager.find(&args.container)?
        .ok_or_else(|| anyhow::anyhow!("Container not found: {}", args.container))?;
    
    // Check if container is already running
    if metadata.is_running() {
        println!("Container {} is already running", metadata.name);
        return Ok(());
    }
    
    // Refresh status to make sure it's actually stopped
    manager.refresh_status(&mut metadata)?;
    
    if metadata.is_running() {
        println!("Container {} is already running", metadata.name);
        return Ok(());
    }
    
    println!("Starting container: {}", metadata.name);
    
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
    
    println!("Container {} started (PID: {})", metadata.name, pid);
    
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
