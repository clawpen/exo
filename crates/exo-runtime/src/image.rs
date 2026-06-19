//! OCI image management and registry operations.
//!
//! This module handles pulling images from registries, managing layers,
//! and parsing OCI/Docker image manifests.

use crate::storage::OverlayfsDriver;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

/// Docker registry default host.
pub const DEFAULT_REGISTRY: &str = "registry-1.docker.io";

/// Docker Hub library.
pub const DEFAULT_LIBRARY: &str = "library";

/// Images directory.
pub const IMAGES_DIR: &str = "/tmp/exo-images";

/// OCI image manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OciManifest {
    /// Schema version
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,

    /// Media type
    #[serde(rename = "mediaType")]
    pub media_type: Option<String>,

    /// Image configuration
    pub config: ImageDescriptor,

    /// Layers
    pub layers: Vec<ImageDescriptor>,

    /// Annotations
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

/// Descriptor for an image component (config or layer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageDescriptor {
    /// Content addressable hash (algorithm:digest)
    #[serde(rename = "digest")]
    pub digest: String,

    /// Media type
    #[serde(rename = "mediaType")]
    pub media_type: String,

    /// Size in bytes
    pub size: i64,

    /// URLs (optional)
    pub urls: Option<Vec<String>>,

    /// Platform (optional)
    pub platform: Option<Platform>,
}

/// Platform information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platform {
    /// Architecture
    pub architecture: String,

    /// OS
    pub os: String,

    /// Variant (optional)
    pub variant: Option<String>,
}

/// Tag or digest reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagOrDigest {
    /// Tag (e.g., "latest", "3.12")
    Tag(String),

    /// Digest (e.g., "sha256:abc123...")
    Digest(String),
}

/// Parsed image reference.
#[derive(Debug, Clone)]
pub struct ParsedImageReference {
    /// Registry host
    pub registry: String,

    /// Repository name
    pub repository: String,

    /// Tag or digest
    pub reference: TagOrDigest,
}

/// Stored image metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredImage {
    /// Image ID
    pub id: String,

    /// Image reference
    pub reference: String,

    /// Manifest digest
    pub manifest_digest: String,

    /// Config layer digest
    pub config_digest: String,

    /// Layer digests
    pub layers: Vec<String>,

    /// Size in bytes
    pub size: u64,

    /// Architecture
    pub architecture: String,

    /// Created timestamp (Unix seconds)
    pub created: i64,
}

/// Image manager for pulling and storing images.
#[derive(Clone)]
pub struct ImageManager {
    /// Storage driver
    storage: OverlayfsDriver,
}

impl ImageManager {
    /// Create a new image manager.
    pub fn new() -> Result<Self> {
        let storage = OverlayfsDriver::new()?;

        Ok(Self {
            storage,
        })
    }

    /// Create an image manager with custom storage.
    pub fn with_storage(storage: OverlayfsDriver) -> Result<Self> {
        Ok(Self {
            storage,
        })
    }

    /// Parse an image reference string.
    pub fn parse_image_reference(&self, reference: &str) -> Result<ParsedImageReference> {
        // Parse format: [registry/]repository[:tag|@digest]
        let parts: Vec<&str> = reference.split('/').collect();

        let (registry, repository) = if parts.len() == 1 {
            // "python:3.12" -> registry-1.docker.io/library/python
            (DEFAULT_REGISTRY.to_string(), format!("{}/{}", DEFAULT_LIBRARY, parts[0]))
        } else if parts.len() == 2 && !parts[0].contains('.') && !parts[0].contains(':') {
            // "library/python:3.12" -> registry-1.docker.io/library/python
            (DEFAULT_REGISTRY.to_string(), parts.join("/"))
        } else if parts[0].contains('.') || parts[0].contains(':') {
            // "docker.io/library/python:3.12" or "my-registry.com/myimage:latest"
            (parts[0].to_string(), parts[1..].join("/"))
        } else {
            // "username/python:3.12" -> registry-1.docker.io/username/python
            (DEFAULT_REGISTRY.to_string(), parts.join("/"))
        };

        // Split repository and tag/digest
        // Check for @digest BEFORE :tag since sha256: contains a colon
        let (repository, reference) = if let Some(pos) = repository.rfind('@') {
            let repo = &repository[..pos];
            let ref_part = &repository[pos + 1..];
            (repo.to_string(), TagOrDigest::Digest(ref_part.to_string()))
        } else if let Some(pos) = repository.rfind(':') {
            let repo = &repository[..pos];
            let ref_part = &repository[pos + 1..];

            if ref_part.starts_with("sha256:") {
                (repo.to_string(), TagOrDigest::Digest(ref_part.to_string()))
            } else {
                (repo.to_string(), TagOrDigest::Tag(ref_part.to_string()))
            }
        } else {
            (repository.clone(), TagOrDigest::Tag("latest".to_string()))
        };

        Ok(ParsedImageReference {
            registry,
            repository,
            reference,
        })
    }

    /// Store image metadata.
    pub fn store_image_metadata(
        &self,
        image_ref: &ParsedImageReference,
        manifest: &OciManifest,
        layer_ids: &[String],
    ) -> Result<String> {
        let images_dir = PathBuf::from(IMAGES_DIR);
        fs::create_dir_all(&images_dir)?;

        let image_id = Uuid::new_v4().to_string();
        let image_path = images_dir.join(&image_id);

        let full_reference = format!("{}/{}:{}",
            image_ref.registry, image_ref.repository,
            match &image_ref.reference {
                TagOrDigest::Tag(t) => t.clone(),
                TagOrDigest::Digest(d) => d.clone(),
            }
        );

        let stored = StoredImage {
            id: image_id.clone(),
            reference: full_reference,
            manifest_digest: manifest.config.digest.clone(),
            config_digest: manifest.config.digest.clone(),
            layers: layer_ids.to_vec(),
            size: manifest.layers.iter().map(|l| l.size as u64).sum(),
            architecture: manifest.config.platform
                .as_ref()
                .map(|p| p.architecture.clone())
                .unwrap_or_else(|| "amd64".to_string()),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        };

        let json = serde_json::to_string_pretty(&stored)?;
        fs::write(&image_path, json)?;

        Ok(image_id)
    }

    /// List all stored images.
    pub fn list_images(&self) -> Result<Vec<StoredImage>> {
        let images_dir = PathBuf::from(IMAGES_DIR);

        if !images_dir.exists() {
            return Ok(Vec::new());
        }

        let mut images = Vec::new();

        for entry in fs::read_dir(&images_dir)? {
            let entry = entry?;
            let path = entry.path();

            if let Ok(json) = fs::read_to_string(&path) {
                if let Ok(image) = serde_json::from_str::<StoredImage>(&json) {
                    images.push(image);
                }
            }
        }

        Ok(images)
    }

    /// Get an image by ID.
    pub fn get_image(&self, id: &str) -> Option<StoredImage> {
        let image_path = PathBuf::from(IMAGES_DIR).join(id);

        if !image_path.exists() {
            return None;
        }

        let json = fs::read_to_string(&image_path).ok()?;
        serde_json::from_str::<StoredImage>(&json).ok()
    }

    /// Remove an image.
    pub fn remove_image(&self, id: &str) -> Result<()> {
        let image = self.get_image(id)
            .ok_or_else(|| anyhow::anyhow!("Image not found: {}", id))?;

        // Remove layers if not shared
        for layer_id in &image.layers {
            let _ = self.storage.remove_layer(layer_id);
        }

        // Remove image metadata
        let image_path = PathBuf::from(IMAGES_DIR).join(id);
        fs::remove_file(&image_path)?;

        tracing::info!("Removed image {}", id);

        Ok(())
    }

    /// Get the root filesystem for an image.
    pub fn get_image_rootfs(&self, id: &str) -> Result<PathBuf> {
        let image = self.get_image(id)
            .ok_or_else(|| anyhow::anyhow!("Image not found: {}", id))?;

        // Create a temporary overlay with the image's layers
        let container_id = format!("{}_rootfs", id);
        let overlay = self.storage.create_container_overlay(&container_id, image.layers.clone())?;

        // Mount the overlay
        #[cfg(target_os = "linux")]
        {
            self.storage.mount(&overlay)?;
        }

        Ok(overlay.merged)
    }

    /// Get the storage driver.
    pub fn storage(&self) -> &OverlayfsDriver {
        &self.storage
    }
}

impl Default for ImageManager {
    fn default() -> Self {
        Self::new().expect("Failed to create image manager")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_reference() {
        let manager = ImageManager::new().unwrap();
        let parsed = manager.parse_image_reference("python:3.12").unwrap();

        assert_eq!(parsed.registry, DEFAULT_REGISTRY);
        assert_eq!(parsed.repository, "library/python");
        assert_eq!(parsed.reference, TagOrDigest::Tag("3.12".to_string()));
    }

    #[test]
    fn test_parse_docker_hub_reference() {
        let manager = ImageManager::new().unwrap();
        let parsed = manager.parse_image_reference("docker.io/library/python:latest").unwrap();

        assert_eq!(parsed.registry, "docker.io");
        assert_eq!(parsed.repository, "library/python");
        assert_eq!(parsed.reference, TagOrDigest::Tag("latest".to_string()));
    }

    #[test]
    fn test_parse_digest_reference() {
        let manager = ImageManager::new().unwrap();
        let parsed = manager.parse_image_reference("python@sha256:abc123").unwrap();

        assert_eq!(parsed.reference, TagOrDigest::Digest("sha256:abc123".to_string()));
    }

    #[test]
    fn test_parse_private_registry() {
        let manager = ImageManager::new().unwrap();
        let parsed = manager.parse_image_reference("my-registry.com/myimage:latest").unwrap();

        assert_eq!(parsed.registry, "my-registry.com");
        assert_eq!(parsed.repository, "myimage");
    }
}
