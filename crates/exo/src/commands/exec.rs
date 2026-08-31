//! Execute command in container

pub struct ExecArgs {
    pub container: String,
    pub command: Vec<String>,
    pub interactive: bool,
    pub tty: bool,
    pub user: Option<String>,
    pub backend: String,
    pub json: bool,
}

pub async fn execute(args: ExecArgs) -> anyhow::Result<()> {
    if args.command.is_empty() {
        return Err(exo_runtime::ExoError::InvalidInput("no command specified".to_string()).into());
    }

    #[cfg(target_os = "macos")]
    {
        use exo_runtime::{ExecOptions, ExoBackend};

        let code = match super::mac::select_backend(&args.backend)? {
            super::mac::BackendSelection::Native => {
                if args.interactive {
                    tracing::warn!("macOS native backend exec inherits stdin/stdout by default");
                }
                if args.tty {
                    tracing::warn!("macOS native backend does not allocate a new pseudo-TTY");
                }
                super::mac::native_backend()?.exec(&args.container, args.command, args.user)?
            }
            super::mac::BackendSelection::Linux => {
                super::mac::linux_backend()
                    .exec(
                        &args.container,
                        args.command,
                        ExecOptions {
                            user: args.user,
                            interactive: args.interactive,
                            tty: args.tty,
                        },
                    )
                    .await?
            }
        };
        if args.json {
            let mut fields = serde_json::Map::new();
            fields.insert("container".to_string(), args.container.clone().into());
            fields.insert("exit_code".to_string(), code.into());
            super::print_json(fields);
        }
        if code != 0 {
            return Err(exo_runtime::ExoError::ContainerExited {
                name: args.container.clone(),
                code,
            }
            .into());
        }
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Fail loudly per the agent contract: a placeholder that returns Ok
        // tells the caller the command ran when nothing happened (roadmap D1).
        Err(exo_runtime::ExoError::BackendUnsupported {
            feature: "exec".to_string(),
            backend: std::env::consts::OS.to_string(),
        }
        .into())
    }
}
