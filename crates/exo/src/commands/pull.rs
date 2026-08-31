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

    // Parse the image reference (typed INVALID_INPUT on malformed refs)
    let image_ref = ImageReference::parse(&args.image)?;

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

    // Create registry client and pull. Registry failures are typed in
    // exo-image (ImageNotFound / RegistryAuth / RegistryUnavailable) and
    // survive the anyhow boundary via downcast — no string sniffing here.
    let mut client = RegistryClient::new(store)?;
    let pulled = client.pull(&image_ref).await?;

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
