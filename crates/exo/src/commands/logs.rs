//! View container logs command

pub struct LogsArgs {
    pub container: String,
    pub follow: bool,
    pub tail: usize,
    pub timestamps: bool,
}

pub async fn execute(args: LogsArgs) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        return execute_macos(args).await;
    }

    #[cfg(not(target_os = "macos"))]
    {
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
}

#[cfg(target_os = "macos")]
async fn execute_macos(args: LogsArgs) -> anyhow::Result<()> {
    let output = super::mac::backend()?.logs(
        &args.container,
        exo_mac::LogOptions {
            follow: args.follow,
            tail: args.tail,
            timestamps: args.timestamps,
        },
    )?;
    print!("{}", output);
    Ok(())
}
