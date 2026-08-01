//! View container logs command

pub struct LogsArgs {
    pub container: String,
    pub follow: bool,
    pub tail: usize,
    pub timestamps: bool,
    pub backend: String,
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
    use exo_runtime::{BackendLogOptions, ExoBackend};

    match super::mac::select_backend(&args.backend)? {
        super::mac::BackendSelection::Native => {
            let output = super::mac::native_backend()?.logs(
                &args.container,
                exo_mac::LogOptions {
                    follow: args.follow,
                    tail: args.tail,
                    timestamps: args.timestamps,
                },
            )?;
            print!("{}", output);
        }
        super::mac::BackendSelection::Linux => {
            let output = super::mac::linux_backend()
                .logs(
                    &args.container,
                    BackendLogOptions {
                        follow: args.follow,
                        tail: args.tail,
                        timestamps: args.timestamps,
                    },
                )
                .await?;
            print!("{}", output.content);
        }
    }
    Ok(())
}
