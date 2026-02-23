//! Execute command in container

pub struct ExecArgs {
    pub container: String,
    pub command: Vec<String>,
    pub interactive: bool,
    pub tty: bool,
    pub user: Option<String>,
}

pub async fn execute(args: ExecArgs) -> anyhow::Result<()> {
    if args.command.is_empty() {
        anyhow::bail!("No command specified");
    }

    println!("Executing in container {}: {:?}", args.container, args.command);

    if args.interactive {
        println!("Interactive mode enabled");
    }

    if args.tty {
        println!("TTY enabled");
    }

    if let Some(user) = &args.user {
        println!("Running as user: {}", user);
    }

    Ok(())
}
