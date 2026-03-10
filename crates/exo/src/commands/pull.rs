//! Pull image command

use exo_image::{ImageReference, RegistryClient, ImageStore};

pub struct PullArgs {
    pub image: String,
}

pub async fn execute(args: PullArgs) -> anyhow::Result<()> {
    println!("Pulling image: {}", args.image);

    // Parse the image reference
    let image_ref = ImageReference::parse(&args.image)?;
    println!("  Registry: {}", image_ref.registry);
    println!("  Repository: {}", image_ref.repository);
    println!("  Tag: {}", image_ref.tag);
    if let Some(ref digest) = image_ref.digest {
        println!("  Digest: {}", digest);
    }

    // Create image store
    let store = ImageStore::default();
    
    // Check if already pulled
    if store.has_image(&image_ref) {
        println!("  Image already exists locally");
        return Ok(());
    }

    // Create registry client and pull
    let mut client = RegistryClient::new(store)?;
    let pulled = client.pull(&image_ref).await?;
    
    println!("  Config: {}", pulled.config_digest);
    println!("  Layers: {}", pulled.layer_digests.len());
    
    println!("\nSuccessfully pulled {}", args.image);

    Ok(())
}
