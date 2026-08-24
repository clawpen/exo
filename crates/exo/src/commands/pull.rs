//! Pull image command

use exo_image::{ImageReference, ImageStore, RegistryClient};

pub struct PullArgs {
    pub image: String,
    pub json: bool,
}

pub async fn execute(args: PullArgs) -> anyhow::Result<()> {
    if !args.json {
        println!("Pulling image: {}", args.image);
    }

    // Parse the image reference
    let image_ref = ImageReference::parse(&args.image).map_err(|e| {
        exo_runtime::ExoError::InvalidInput(format!("invalid image reference '{}': {e}", args.image))
    })?;

    // Create image store
    let store = ImageStore::default();

    // Check if already pulled
    if store.has_image(&image_ref) {
        if args.json {
            let mut fields = serde_json::Map::new();
            fields.insert("image".to_string(), args.image.clone().into());
            fields.insert("cached".to_string(), true.into());
            super::print_json(fields);
        } else {
            println!("  Image already exists locally");
        }
        return Ok(());
    }

    if !args.json {
        println!("  Registry: {}", image_ref.registry);
        println!("  Repository: {}", image_ref.repository);
        println!("  Tag: {}", image_ref.tag);
        if let Some(ref digest) = image_ref.digest {
            println!("  Digest: {}", digest);
        }
    }

    // Create registry client and pull
    let mut client = RegistryClient::new(store)?;
    // Transitional: exo-image still returns stringly registry errors with the
    // HTTP status embedded; map 404s to the typed taxonomy until exo-image
    // gets its own typed errors (tracked in ROADMAP A1 follow-up).
    let pulled = client.pull(&image_ref).await.map_err(|e| {
        if e.to_string().contains("404") {
            exo_runtime::ExoError::ImageNotFound(args.image.clone()).into()
        } else {
            e
        }
    })?;

    if args.json {
        let mut fields = serde_json::Map::new();
        fields.insert("image".to_string(), args.image.clone().into());
        fields.insert("cached".to_string(), false.into());
        fields.insert("config_digest".to_string(), pulled.config_digest.into());
        fields.insert("layers".to_string(), pulled.layer_digests.len().into());
        super::print_json(fields);
    } else {
        println!("  Config: {}", pulled.config_digest);
        println!("  Layers: {}", pulled.layer_digests.len());
        println!("\nSuccessfully pulled {}", args.image);
    }

    Ok(())
}
