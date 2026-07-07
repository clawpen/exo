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

    #[cfg(target_os = "macos")]
    {
        if args.interactive {
            tracing::warn!("macOS native backend exec inherits stdin/stdout by default");
        }
        if args.tty {
            tracing::warn!("macOS native backend does not allocate a new pseudo-TTY");
        }
        let code = super::mac::backend()?.exec(&args.container, args.command, args.user)?;
        if code != 0 {
            anyhow::bail!("exec exited with code {}", code);
        }
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        println!(
            "Executing in container {}: {:?}",
            args.container, args.command
        );

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
}
