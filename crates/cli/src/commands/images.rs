//! List images command

use containment_runtime::ImageManager;
use chrono::{DateTime, Utc};

pub struct ImagesArgs {
    pub all: bool,
}

pub async fn execute(args: ImagesArgs) -> anyhow::Result<()> {
    let image_manager = ImageManager::new()?;

    let images = image_manager.list_images()?;

    if images.is_empty() {
        println!("No images found.");
        return Ok(());
    }

    println!("{:<20} {:<15} {:<12} {:<12} {}", "REPOSITORY", "TAG", "IMAGE ID", "SIZE", "CREATED");
    println!("{}", "-".repeat(70));

    for img in images {
        // Parse reference to get repo and tag
        let parts: Vec<&str> = img.reference.split('/').collect();
        let (repo, tag) = if parts.len() >= 2 {
            let repo_and_tag = *parts.last().unwrap();
            if let Some(pos) = repo_and_tag.find(':') {
                (&repo_and_tag[..pos], &repo_and_tag[pos + 1..])
            } else {
                (repo_and_tag, "latest")
            }
        } else {
            (parts[0], "latest")
        };

        let size_mb = img.size / (1024 * 1024);
        let size_str = if size_mb > 1024 {
            format!("{}GB", size_mb / 1024)
        } else if size_mb > 0 {
            format!("{}MB", size_mb)
        } else {
            "<1MB".to_string()
        };

        let created = DateTime::<Utc>::from_timestamp(img.created, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        println!("{:<20} {:<15} {:<12} {:>12} {}",
            repo, tag, &img.id[..12], size_str, created);
    }

    Ok(())
}
