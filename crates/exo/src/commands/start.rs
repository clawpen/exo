//! Start container command - restart a stopped container

use exo_runtime::ContainerManager;
use exo_runtime::{Container, ContainerStatus};
use anyhow::Result;

pub struct StartArgs {
    pub container: String,
    /// Attach to container (follow logs)
    pub attach: bool,
}

pub async fn execute(args: StartArgs) -> Result<()> {
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
