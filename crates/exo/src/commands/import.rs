//! Import image from tarball command

use anyhow::Context;
use anyhow::Result;
use exo_image::{ImageReference, ImageStore};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

pub struct ImportArgs {
    pub tarball: PathBuf,
    pub name: Option<String>, // Optional: rename image on import
}

pub async fn execute(args: ImportArgs) -> Result<()> {
    println!("Importing image from: {}", args.tarball.display());

    // Validate tarball exists
    if !args.tarball.exists() {
        anyhow::bail!("Tarball not found: {}", args.tarball.display());
    }

    // Create image store
    let store = ImageStore::default();

    // Extract tarball to temp directory
    let temp_dir = tempfile::tempdir()?;

    // Extract the outer tarball
    {
        let file = File::open(&args.tarball).context("Failed to open tarball")?;

        let is_gzipped = args.tarball.extension().map(|e| e == "gz").unwrap_or(false);

        if is_gzipped {
            let decoder = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);
            archive
                .unpack(temp_dir.path())
                .context("Failed to extract gzipped tarball")?;
        } else {
            let mut archive = tar::Archive::new(file);
            archive
                .unpack(temp_dir.path())
                .context("Failed to extract tarball")?;
        }
    }

    // Read manifest.json
    let manifest_path = temp_dir.path().join("manifest.json");
    let manifest_data = std::fs::read(&manifest_path).context("Failed to read manifest.json")?;

    let manifest: Vec<serde_json::Value> =
        serde_json::from_slice(&manifest_data).context("Failed to parse manifest.json")?;

    let first_image = manifest.first().context("Empty manifest")?;

    // Extract RepoTags
    let mut repo_tags: Vec<String> = vec![];
    if let Some(tags) = first_image.get("RepoTags").and_then(|t| t.as_array()) {
        for tag in tags {
            if let Some(tag_str) = tag.as_str() {
                repo_tags.push(tag_str.to_string());
            }
        }
    }

    println!("  Found {} tags", repo_tags.len());
    for tag in &repo_tags {
        println!("    - {}", tag);
    }

    // Read config
    let config_file = first_image
        .get("Config")
        .and_then(|c| c.as_str())
        .context("No config in manifest")?;

    let config_path = temp_dir.path().join(config_file);
    let config_data = std::fs::read(&config_path).context("Failed to read config file")?;
    let config: serde_json::Value =
        serde_json::from_slice(&config_data).context("Failed to parse config JSON")?;

    // Extract layers
    let layer_digests = first_image
        .get("Layers")
        .and_then(|l| l.as_array())
        .context("No layers in manifest")?;

    println!("  Extracting {} layers...", layer_digests.len());

    // Determine image reference
    let image_ref = if let Some(name) = &args.name {
        ImageReference::parse(name)?
    } else if let Some(tag) = repo_tags.first() {
        ImageReference::parse(tag)?
    } else {
        anyhow::bail!("No image name found. Use --name to specify.");
    };

    let rootfs_path = store.rootfs_path(&image_ref);
    std::fs::create_dir_all(&rootfs_path)?;

    // Extract each layer to rootfs
    for (i, layer_path) in layer_digests.iter().enumerate() {
        let layer_path = layer_path.as_str().context("Invalid layer path")?;
        let full_path = temp_dir.path().join(layer_path);

        println!(
            "    Layer {}/{}: {}",
            i + 1,
            layer_digests.len(),
            layer_path
        );

        // Extract layer tar to rootfs (overlaying)
        let layer_file = File::open(&full_path)
            .with_context(|| format!("Failed to open layer: {}", layer_path))?;

        // Try to detect if layer is gzipped
        let mut layer_reader = std::io::BufReader::new(layer_file);
        let mut magic = [0u8; 2];
        let bytes_read = layer_reader.read(&mut magic)?;

        // Reopen the file since we read the magic bytes
        let layer_file = File::open(&full_path)?;
        let is_gzipped = bytes_read >= 2 && magic[0] == 0x1f && magic[1] == 0x8b;

        if is_gzipped {
            let decoder = flate2::read::GzDecoder::new(layer_file);
            let mut layer_archive = tar::Archive::new(decoder);
            layer_archive
                .unpack(&rootfs_path)
                .with_context(|| format!("Failed to extract layer: {}", layer_path))?;
        } else {
            let mut layer_archive = tar::Archive::new(layer_file);
            layer_archive
                .unpack(&rootfs_path)
                .with_context(|| format!("Failed to extract layer: {}", layer_path))?;
        }
    }

    // Save manifest for exo
    let manifest_path = store.manifest_path(&image_ref);
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&config)?)?;

    println!("\nSuccessfully imported: {}", image_ref);
    println!("  Rootfs: {}", rootfs_path.display());

    Ok(())
}
