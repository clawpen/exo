//! View container logs command

pub struct LogsArgs {
    pub container: String,
    pub follow: bool,
    pub tail: usize,
    pub timestamps: bool,
}

pub async fn execute(args: LogsArgs) -> anyhow::Result<()> {
    println!("Showing logs for container: {}", args.container);

    if args.follow {
        println!("Following logs...");
    }

    if args.timestamps {
        println!("With timestamps");
    }

    println!("(Last {} lines)", args.tail);

    Ok(())
}
