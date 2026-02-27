//! Remove container command

pub struct RemoveArgs {
    pub container: String,
    pub force: bool,
}

pub async fn execute(args: RemoveArgs) -> anyhow::Result<()> {
    println!("Removing container: {}", args.container);

    if args.force {
        println!("Force removing (will stop if running)");
    }

    Ok(())
}
