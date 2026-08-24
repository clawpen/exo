//! List images command

use exo_image::ImageManager;

pub struct ImagesArgs {
    pub all: bool,
    pub json: bool,
}

pub async fn execute(args: ImagesArgs) -> anyhow::Result<()> {
    let image_manager = ImageManager::new()?;

    let images = image_manager.list_images()?;

    if args.json {
        let list: Vec<serde_json::Value> = images
            .iter()
            .map(|img| {
                serde_json::json!({
                    "repository": img.repository,
                    "tag": img.tag,
                    "registry": img.registry,
                })
            })
            .collect();
        let mut fields = serde_json::Map::new();
        fields.insert("images".to_string(), serde_json::Value::Array(list));
        super::print_json(fields);
        return Ok(());
    }

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

        println!("{:<30} {:<15} {:<20}", repo, img.tag, img.registry);
    }

    Ok(())
}
