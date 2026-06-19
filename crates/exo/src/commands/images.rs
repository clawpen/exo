//! List images command

use exo_image::ImageManager;

pub struct ImagesArgs {
    pub all: bool,
}

pub async fn execute(_args: ImagesArgs) -> anyhow::Result<()> {
    let image_manager = ImageManager::new()?;

    let images = image_manager.list_images()?;

    if images.is_empty() {
        println!("No images found.");
        return Ok(());
    }

    println!("{:<30} {:<15} {:<20}", "REPOSITORY", "TAG", "REGISTRY");
    println!("{}", "-".repeat(70));

    for img in images {
        // Extract repo name from repository path
        let repo = if img.repository.contains('/') {
            img.repository.split('/').last().unwrap_or(&img.repository)
        } else {
            &img.repository
        };

        println!("{:<30} {:<15} {:<20}",
            repo, img.tag, img.registry);
    }

    Ok(())
}
