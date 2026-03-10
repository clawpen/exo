//! Stop container command

use exo_runtime::ContainerManager;
use anyhow::Result;

pub struct StopArgs {
    pub container: String,
    pub force: bool,
    pub time: u64,
}

pub async fn execute(args: StopArgs) -> Result<()> {
    let manager = ContainerManager::new()?;
    
    // Find container by name or ID
    let mut metadata = manager.find(&args.container)?
        .ok_or_else(|| anyhow::anyhow!("Container not found: {}", args.container))?;
    
    // Check if container is running
    if !metadata.is_running() {
        println!("Container {} is not running (status: {})", metadata.name, metadata.status);
        return Ok(());
    }
    
    let pid = metadata.pid
        .ok_or_else(|| anyhow::anyhow!("Container has no PID"))?;
    
    // Send signal to stop the container
    if args.force {
        println!("Force stopping container: {}", metadata.name);
        // Send SIGKILL
        send_signal(pid, 9)?;
    } else {
        println!("Stopping container: {} (waiting {}s)", metadata.name, args.time);
        // Send SIGTERM
        send_signal(pid, 15)?;
        
        // Wait for process to exit
        let waited = wait_for_exit(pid, args.time);
        if !waited {
            println!("Container did not stop gracefully, sending SIGKILL");
            send_signal(pid, 9)?;
            // Give it a moment to die
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    
    // Update metadata
    metadata.set_stopped(None);
    manager.save(&metadata)?;
    
    println!("Container {} stopped", metadata.name);
    
    Ok(())
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
