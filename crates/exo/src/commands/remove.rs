//! Remove container command

use exo_runtime::ContainerManager;
use anyhow::Result;

pub struct RemoveArgs {
    pub container: String,
    pub force: bool,
}

pub async fn execute(args: RemoveArgs) -> Result<()> {
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
