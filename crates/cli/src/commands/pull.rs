//! Pull image command

use containment_runtime::ImageManager;

pub struct PullArgs {
    pub image: String,
}

pub async fn execute(args: PullArgs) -> anyhow::Result<()> {
    println!("Pulling image: {}", args.image);

    // Initialize image manager
    let image_manager = ImageManager::new()?;

    // Parse the image reference
    let image_ref = image_manager.parse_image_reference(&args.image)?;
    println!("  Registry: {}", image_ref.registry);
    println!("  Repository: {}", image_ref.repository);
    println!("  Reference: {:?}", image_ref.reference);

    // Check if image is registry pull
    #[cfg(feature = "registry")]
    {
        let digest = image_manager.pull(&args.image).await?;
        println!("  Image digest: {}", digest);
    }

    #[cfg(not(feature = "registry"))]
    {
        println!("  Image registry support not enabled. Use --features registry to enable.");
    }

    // List local images
    let images = image_manager.list_images()?;
    println!("\nLocal images:");
    for img in images {
        println!("  {} - {} ({})", img.id[..12].to_string(), img.reference, img.size);
    }

    Ok(())
}
