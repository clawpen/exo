//! Image storage management.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::debug;

use crate::ImageReference;

/// Image store - manages local image storage.
#[derive(Clone)]
pub struct ImageStore {
    root_path: PathBuf,
}

impl ImageStore {
    /// Create a new image store.
    pub fn new(root_path: PathBuf) -> Self {
        std::fs::create_dir_all(root_path.join("blobs")).ok();
        std::fs::create_dir_all(root_path.join("manifests")).ok();
        std::fs::create_dir_all(root_path.join("rootfs")).ok();
        Self { root_path }
    }

    /// Get the default image store path.
    pub fn default_path() -> PathBuf {
        PathBuf::from(crate::DEFAULT_IMAGE_ROOT)
    }

    /// Get the root path of the store.
    pub fn root(&self) -> &std::path::Path {
        &self.root_path
    }

    /// Check if an image exists locally.
    pub fn has_image(&self, reference: &ImageReference) -> bool {
        let manifest_path = self.manifest_path(reference);
        manifest_path.exists()
    }

    /// Get the path to an image's manifest.
    pub fn manifest_path(&self, reference: &ImageReference) -> PathBuf {
        self.root_path.join("manifests").join(format!(
            "{}_{}.json",
            reference.repository.replace('/', "_"),
            reference.tag
        ))
    }

    /// Get the path to an image's extracted rootfs.
    pub fn rootfs_path(&self, reference: &ImageReference) -> PathBuf {
        self.root_path.join("rootfs").join(reference.fs_name())
    }

    /// Get the path to a blob.
    pub fn blob_path(&self, digest: &str) -> PathBuf {
        // digest is like "sha256:abc123..."
        let digest = digest.replace(':', "_");
        self.root_path.join("blobs").join(&digest)
    }

    /// Check if a blob exists.
    pub fn has_blob(&self, digest: &str) -> bool {
        self.blob_path(digest).exists()
    }

    /// List all locally stored images.
    pub fn list_images(&self) -> Result<Vec<ImageReference>> {
        let mut images = vec![];
        let manifests_dir = self.root_path.join("manifests");

        if let Ok(entries) = std::fs::read_dir(manifests_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(repo_tag) = name_str.strip_suffix(".json") {
                    // Parse back to ImageReference
                    if let Some((repo, tag)) = repo_tag.rsplit_once('_') {
                        let full = format!("docker.io/{}:{}", repo.replace('_', "/"), tag);
                        if let Ok(reference) = ImageReference::parse(&full) {
                            images.push(reference);
                        }
                    }
                }
            }
        }

        Ok(images)
    }

    /// Save a manifest for an image.
    pub fn save_manifest(&self, reference: &ImageReference, manifest: &[u8]) -> Result<()> {
        let path = self.manifest_path(reference);
        std::fs::write(&path, manifest)
            .with_context(|| format!("Failed to save manifest to {:?}", path))?;
        debug!("Saved manifest for {} to {:?}", reference, path);
        Ok(())
    }

    /// Save an OCI manifest for an image.
    pub fn save_oci_manifest(
        &self,
        reference: &ImageReference,
        manifest: &oci_spec::image::ImageManifest,
    ) -> Result<()> {
        let path = self.manifest_path(reference);
        let json = serde_json::to_vec_pretty(manifest).context("Failed to serialize manifest")?;
        std::fs::write(&path, &json)
            .with_context(|| format!("Failed to save manifest to {:?}", path))?;
        debug!("Saved OCI manifest for {} to {:?}", reference, path);
        Ok(())
    }

    /// Load a manifest for an image.
    pub fn load_manifest(&self, reference: &ImageReference) -> Result<Vec<u8>> {
        let path = self.manifest_path(reference);
        std::fs::read(&path).with_context(|| format!("Failed to load manifest from {:?}", path))
    }

    /// Remove an image from the store.
    pub fn remove_image(&self, reference: &ImageReference) -> Result<()> {
        let manifest = self.manifest_path(reference);
        let rootfs = self.rootfs_path(reference);

        if manifest.exists() {
            std::fs::remove_file(&manifest)?;
        }

        if rootfs.exists() {
            std::fs::remove_dir_all(&rootfs)?;
        }

        debug!("Removed image {}", reference);
        Ok(())
    }

    /// Clean up a container overlay (if any).
    pub fn remove_container_overlay(&self, _container_id: &str) -> Result<()> {
        // This is handled by the storage layer, not the image store
        Ok(())
    }
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}
