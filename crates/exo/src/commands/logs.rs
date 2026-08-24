//! View container logs command

pub struct LogsArgs {
    pub container: String,
    pub follow: bool,
    pub tail: usize,
    pub timestamps: bool,
    pub backend: String,
    pub json: bool,
}

pub async fn execute(args: LogsArgs) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        return execute_macos(args).await;
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Fail loudly per the agent contract: a placeholder that prints
        // flags and returns Ok fakes success (roadmap D1).
        Err(exo_runtime::ExoError::BackendUnsupported {
            feature: "logs".to_string(),
            backend: std::env::consts::OS.to_string(),
        }
        .into())
    }
}

#[cfg(target_os = "macos")]
async fn execute_macos(args: LogsArgs) -> anyhow::Result<()> {
    use exo_runtime::{BackendLogOptions, ExoBackend};

    let content = match super::mac::select_backend(&args.backend)? {
        super::mac::BackendSelection::Native => super::mac::native_backend()?.logs(
            &args.container,
            exo_mac::LogOptions {
                follow: args.follow,
                tail: args.tail,
                timestamps: args.timestamps,
            },
        )?,
        super::mac::BackendSelection::Linux => {
            super::mac::linux_backend()
                .logs(
                    &args.container,
                    BackendLogOptions {
                        follow: args.follow,
                        tail: args.tail,
                        timestamps: args.timestamps,
                    },
                )
                .await?
                .content
        }
    };

    if args.json {
        let mut fields = serde_json::Map::new();
        fields.insert("container".to_string(), args.container.clone().into());
        fields.insert("content".to_string(), content.into());
        super::print_json(fields);
    } else {
        print!("{}", content);
    }
    Ok(())
}
