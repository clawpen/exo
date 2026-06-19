//! Exo Image Management
//!
//! OCI image format support: pulling, extraction, and storage.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

pub use oci_spec::image::{ImageManifest, MediaType};

mod reference;
mod store;
mod extractor;
mod cas;
mod manifest;
mod oci_build;
mod ignore;

pub use reference::ImageReference;
pub use store::ImageStore;
pub use extractor::LayerExtractor;
pub use cas::{DiskUsage, ImageIndex, ImageInfo, ImageRecord, LayerInfo, LayerStore, StoreReport};
pub use manifest::{
    AgentManifest, AgentSection, BuildSection, EgressSection, ResourcesSection, ToolsSection,
};
pub use oci_build::build_and_store;
pub use ignore::ExoIgnore;

#[cfg(feature = "registry")]
mod registry;

#[cfg(feature = "registry")]
mod puller;

#[cfg(feature = "registry")]
pub use registry::{RegistryClient, RegistryAuth, PulledImage};

#[cfg(feature = "registry")]
pub use puller::ImagePuller;

/// Default image storage location
pub const DEFAULT_IMAGE_ROOT: &str = "/tmp/exo-images";

/// Parse an image reference from a string.
pub fn parse_image(s: &str) -> Result<ImageReference> {
    ImageReference::parse(s)
}

/// Get the rootfs path for an image.
pub fn get_rootfs_path(image_name: &str) -> PathBuf {
    PathBuf::from(DEFAULT_IMAGE_ROOT)
        .join("rootfs")
        .join(image_name.replace(['/', ':'], "_"))
}

/// Check if an image's rootfs is already extracted.
pub fn is_rootfs_ready(image_name: &str) -> bool {
    let rootfs = get_rootfs_path(image_name);
    rootfs.exists() && rootfs.join("bin").exists()
}

/// Extract a single layer to the rootfs, handling whiteouts.
pub fn extract_layer(layer_tar: &Path, dest: &Path) -> Result<()> {
    debug!("Extracting layer {:?} to {:?}", layer_tar, dest);
    
    let file = std::fs::File::open(layer_tar)
        .with_context(|| format!("Failed to open layer: {:?}", layer_tar))?;
    
    // OCI/Docker layers are typically gzipped, but may not have .gz extension
    // Try to detect gzip magic bytes
    let mut reader = std::io::BufReader::new(file);
    let mut magic = [0u8; 2];
    let bytes_read = reader.read(&mut magic)?;
    
    // Reset reader position by reopening the file
    let file = std::fs::File::open(layer_tar)?;
    
    let is_gzipped = bytes_read >= 2 && magic[0] == 0x1f && magic[1] == 0x8b;
    
    let reader: Box<dyn std::io::Read> = if is_gzipped {
        debug!("Detected gzip compression for layer");
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    
    let mut archive = tar::Archive::new(reader);
    
    // First pass: collect whiteouts
    let mut whiteouts: Vec<PathBuf> = Vec::new();
    let mut opaque_dirs: Vec<PathBuf> = Vec::new();
    
    // We need to collect paths first, then re-open for extraction
    // Store entry paths and metadata
    let mut entry_paths: Vec<(PathBuf, bool)> = Vec::new(); // (path, is_whiteout)
    
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?.to_path_buf();
        let file_name = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        
        if file_name.starts_with(".wh.") {
            if file_name == ".wh..wh..opq" {
                opaque_dirs.push(path.parent().unwrap().to_path_buf());
            } else {
                let original_name = file_name.strip_prefix(".wh.").unwrap();
                let parent = path.parent().unwrap();
                whiteouts.push(parent.join(original_name));
            }
            entry_paths.push((path, true));
        } else {
            entry_paths.push((path, false));
        }
    }
    
    // Handle opaque directories (clear existing content)
    for opaque_dir in &opaque_dirs {
        let full_path = dest.join(opaque_dir);
        if full_path.exists() {
            debug!("Clearing opaque directory: {:?}", full_path);
            std::fs::remove_dir_all(&full_path).ok();
            std::fs::create_dir_all(&full_path)?;
        }
    }
    
    // Handle whiteouts (delete files/dirs)
    for whiteout in &whiteouts {
        let full_path = dest.join(whiteout);
        if full_path.exists() {
            debug!("Applying whiteout: {:?}", full_path);
            if full_path.is_dir() {
                std::fs::remove_dir_all(&full_path).ok();
            } else {
                std::fs::remove_file(&full_path).ok();
            }
        }
    }
    
    // Re-open archive for extraction
    let file = std::fs::File::open(layer_tar)?;
    let reader: Box<dyn std::io::Read> = if is_gzipped {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    
    let mut archive = tar::Archive::new(reader);
    
    // Second pass: extract files (skip whiteout files)
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let file_name = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        
        // Skip whiteout files
        if file_name.starts_with(".wh.") {
            continue;
        }
        
        let dest_path = dest.join(&path);
        
        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // Extract the file - use unpack_in to handle hard links properly
        entry.unpack_in(dest)?;
    }
    
    Ok(())
}

/// Prepare a minimal Alpine rootfs for testing.
/// Downloads the minirootfs if not present.
#[cfg(feature = "registry")]
pub async fn prepare_alpine_rootfs(dest: &Path) -> Result<()> {
    if dest.exists() && dest.join("bin").exists() {
        debug!("Alpine rootfs already exists at {:?}", dest);
        return Ok(());
    }
    
    info!("Downloading Alpine minirootfs...");
    
    let url = "https://dl-cdn.alpinelinux.org/alpine/v3.19/releases/x86_64/alpine-minirootfs-3.19.0-x86_64.tar.gz";
    
    let response = reqwest::get(url).await?;
    
    if !response.status().is_success() {
        anyhow::bail!("Failed to download Alpine rootfs: {}", response.status());
    }
    
    let bytes = response.bytes().await?;
    
    // Save to temp file
    let temp_tar = dest.parent().unwrap().join("alpine-minirootfs.tar.gz");
    std::fs::create_dir_all(temp_tar.parent().unwrap())?;
    std::fs::write(&temp_tar, &bytes)?;
    
    info!("Extracting Alpine rootfs to {:?}", dest);
    std::fs::create_dir_all(dest)?;
    
    // Extract
    let file = std::fs::File::open(&temp_tar)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dest)?;
    
    // Cleanup
    std::fs::remove_file(&temp_tar)?;
    
    info!("Alpine rootfs ready");
    Ok(())
}

/// Simple image manager for the runtime.
pub struct ImageManager {
    store: ImageStore,
}

impl ImageManager {
    pub fn new() -> Result<Self> {
        let store = ImageStore::new(PathBuf::from(DEFAULT_IMAGE_ROOT));
        Ok(Self { store })
    }
    
    /// Get the storage backend
    pub fn storage(&self) -> &ImageStore {
        &self.store
    }
    
    /// Parse an image reference
    pub fn parse_image_reference(&self, image: &str) -> Result<ImageReference> {
        ImageReference::parse(image)
    }
    
    /// Ensure an image's rootfs is ready for use.
    /// Returns the path to the rootfs.
    #[cfg(feature = "registry")]
    pub async fn ensure_rootfs(&self, image: &ImageReference) -> Result<PathBuf> {
        let rootfs = self.store.rootfs_path(image);
        
        if rootfs.exists() && rootfs.join("bin").exists() {
            debug!("Rootfs already exists: {:?}", rootfs);
            return Ok(rootfs);
        }
        
        // Try registry pull
        let mut client = RegistryClient::new(self.store.clone())?;
        client.pull(image).await?;
        
        Ok(rootfs)
    }
    
    /// Ensure an image's rootfs is ready for use (no registry support).
    /// Returns the path to the rootfs.
    #[cfg(not(feature = "registry"))]
    pub async fn ensure_rootfs(&self, image: &ImageReference) -> Result<PathBuf> {
        let image_name = format!("{}_{}", image.repository.replace('/', "_"), image.tag);
        let rootfs = get_rootfs_path(&image_name);
        
        if rootfs.exists() && rootfs.join("bin").exists() {
            debug!("Rootfs already exists: {:?}", rootfs);
            return Ok(rootfs);
        }
        
        anyhow::bail!(
            "Image {} not found locally and registry support not compiled in. \
             Recompile with --features registry",
            image
        );
    }
    
    /// Pull an image from a registry.
    #[cfg(feature = "registry")]
    pub async fn pull(&self, image: &str) -> Result<String> {
        let reference = ImageReference::parse(image)?;
        let mut client = RegistryClient::new(self.store.clone())?;
        let pulled = client.pull(&reference).await?;
        Ok(pulled.config_digest)
    }
    
    /// Push a locally-stored image to its registry.
    #[cfg(feature = "registry")]
    pub async fn push(&self, image: &str) -> Result<()> {
        let reference = ImageReference::parse(image)?;
        let mut client = RegistryClient::new(self.store.clone())?;
        client.push(&reference).await
    }

    /// List locally stored images.
    pub fn list_images(&self) -> Result<Vec<LocalImage>> {
        let images = self.store.list_images()?;
        Ok(images.into_iter().map(|r| LocalImage {
            reference: r.to_string(),
            registry: r.registry.clone(),
            repository: r.repository.clone(),
            tag: r.tag.clone(),
        }).collect())
    }
}

impl Default for ImageManager {
    fn default() -> Self {
        Self::new().expect("Failed to create ImageManager")
    }
}

/// Information about a locally stored image.
#[derive(Debug, Clone)]
pub struct LocalImage {
    /// Full reference string.
    pub reference: String,
    /// Registry hostname.
    pub registry: String,
    /// Repository name.
    pub repository: String,
    /// Tag.
    pub tag: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_image() {
        let img = parse_image("alpine:3.19").unwrap();
        assert_eq!(img.registry, "docker.io");
        assert_eq!(img.repository, "library/alpine");
        assert_eq!(img.tag, "3.19");
    }
    
    #[test]
    fn test_parse_image_implicit_latest() {
        let img = parse_image("ubuntu").unwrap();
        assert_eq!(img.tag, "latest");
    }
}
