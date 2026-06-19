//! Push image command — publish a locally-stored image to its registry.

use exo_image::{ImageReference, ImageStore, RegistryClient};

pub struct PushArgs {
    pub image: String,
}

pub async fn execute(args: PushArgs) -> anyhow::Result<()> {
    let image_ref = ImageReference::parse(&args.image)?;
    println!("Pushing {} to {}", image_ref, image_ref.registry);

    let store = ImageStore::default();
    if !store.has_image(&image_ref) {
        anyhow::bail!("{} not found locally; pull or build it first", image_ref);
    }

    let mut client = RegistryClient::new(store)?;
    client.push(&image_ref).await?;

    println!("Successfully pushed {}", args.image);
    Ok(())
}
