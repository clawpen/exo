//! List images command

pub struct ImagesArgs {
    pub all: bool,
}

pub async fn execute(_args: ImagesArgs) -> anyhow::Result<()> {
    println!("REPOSITORY          TAG          IMAGE ID          SIZE");
    println!("{}", "-".repeat(60));

    // TODO: List images from storage
    // 1. Read /var/lib/openclaw/images
    // 2. Parse image metadata
    // 3. Display in formatted table

    Ok(())
}
