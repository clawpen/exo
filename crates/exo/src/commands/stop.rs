//! Stop container command

pub struct StopArgs {
    pub container: String,
    pub force: bool,
    pub time: u64,
}

pub async fn execute(args: StopArgs) -> anyhow::Result<()> {
    // TODO: Implement container lookup and stopping

    if args.force {
        println!("Force stopping container: {}", args.container);
        // Send SIGKILL immediately
    } else {
        println!("Stopping container: {} (waiting {}s)", args.container, args.time);
        // Send SIGTERM, wait, then SIGKILL if needed
    }

    Ok(())
}
