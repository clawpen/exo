//! Overlayfs storage driver for layered container images.
//!
//! This module implements the overlay2 storage driver similar to Docker,
//! allowing containers to share base layers while maintaining separate
//! writable top layers.
//!
//! # Storage Layout
//!
//! ```text
//! /var/lib/openclaw/overlay2/
//!   ├── <layer-id>/          # Each layer directory
//!   │   ├── diff/            # Actual layer content
//!   │   ├── link             # Short link to layer ID
//!   │   └── lower            # Lower layers (if not base)
//!   ├── l/                   # Short links for fast lookup
//!   │   └── <short-id> -> ../../<layer-id>/
//!   └── <container-id>/      # Container working directories
//!       ├── merged/          # Mounted overlayfs (container rootfs)
//!       ├── work/            # Overlay work directory
//!       └── upper/           # Writable layer
//! ```

use anyhow::{Context, Result};
use std::fs::{self, create_dir_all, File, remove_dir_all};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use uuid::Uuid;

#[cfg(target_os = "linux")]
use std::os::unix::fs::symlink;

/// Root directory for overlay2 storage.
pub const OVERLAY2_ROOT: &str = "/tmp/exo-overlay2";

/// Directory containing layer diffs.
pub const DIFF_DIR: &str = "diff";

/// Directory containing short links.
pub const LINK_DIR: &str = "l";

/// Minimum length for short link IDs.
pub const MIN_LINK_ID_LENGTH: usize = 5;

/// Maximum length for short link IDs.
pub const MAX_LINK_ID_LENGTH: usize = 10;

/// Overlayfs storage driver.
#[derive(Debug, Clone)]
pub struct OverlayfsDriver {
    /// Root directory for overlay storage
    root: PathBuf,

    /// Mounted layers tracking
    mounts: Arc<RwLock<Vec<MountInfo>>>,
}

/// Information about a mounted overlay.
#[derive(Debug, Clone)]
pub struct MountInfo {
    /// Container or layer ID
    pub id: String,

    /// Mount point path
    pub target: PathBuf,

    /// Lower layers
    pub lowers: Vec<String>,

    /// Upper (writable) layer
    pub upper: Option<PathBuf>,

    /// Work directory
    pub work: PathBuf,
}

/// Layer descriptor.
#[derive(Debug, Clone)]
pub struct Layer {
    /// Unique layer ID (SHA256 hex)
    pub id: String,

    /// Path to layer diff directory
    pub diff_path: PathBuf,

    /// Short link ID
    pub link_id: String,

    /// Size in bytes
    pub size: u64,

    /// Lower layers (dependencies)
    pub lower: Option<Vec<String>>,

    /// Cache ID (for content-addressable storage)
    pub cache_id: Option<String>,
}

/// Container overlay directories.
#[derive(Debug, Clone)]
pub struct ContainerOverlay {
    /// Container ID
    pub container_id: String,

    /// Merged directory (the container rootfs)
    pub merged: PathBuf,

    /// Upper directory (writable layer)
    pub upper: PathBuf,

    /// Work directory (overlayfs work dir)
    pub work: PathBuf,

    /// Lower layers (read-only)
    pub lowers: Vec<String>,
}

impl OverlayfsDriver {
    /// Create a new overlayfs storage driver.
    pub fn new() -> Result<Self> {
        let root = PathBuf::from(OVERLAY2_ROOT);
        Self::with_root(root)
    }

    /// Create a new overlayfs storage driver with custom root.
    pub fn with_root(root: PathBuf) -> Result<Self> {
        // Ensure root directory exists
        create_dir_all(&root)
            .with_context(|| format!("Failed to create overlay root: {:?}", root))?;

        // Create subdirectories
        let diff_dir = root.join(DIFF_DIR);
        let link_dir = root.join(LINK_DIR);
        create_dir_all(&diff_dir)
            .with_context(|| format!("Failed to create diff directory: {:?}", diff_dir))?;
        create_dir_all(&link_dir)
            .with_context(|| format!("Failed to create link directory: {:?}", link_dir))?;

        Ok(Self {
            root,
            mounts: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Get the root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the diff directory for a layer.
    pub fn layer_diff_path(&self, layer_id: &str) -> PathBuf {
        self.root.join(layer_id).join(DIFF_DIR)
    }

    /// Get the short link path for a layer.
    pub fn layer_link_path(&self, link_id: &str) -> PathBuf {
        self.root.join(LINK_DIR).join(link_id)
    }

    /// Add a new layer to storage.
    ///
    /// Creates the layer directory structure and a short link.
    pub fn add_layer(&self, layer_id: &str, data: &[u8]) -> Result<Layer> {
        let layer_path = self.root.join(layer_id);
        let diff_path = layer_path.join(DIFF_DIR);

        // Create layer directory
        create_dir_all(&diff_path)
            .with_context(|| format!("Failed to create layer directory: {:?}", diff_path))?;

        // Write the layer data (tar/gz extraction would happen here)
        // For now, we just create the structure

        // Generate and create short link
        let link_id = self.generate_link_id(layer_id)?;
        let link_path = self.layer_link_path(&link_id);
        let target_path = format!("../../{}", layer_id);

        #[cfg(target_os = "linux")]
        {
            symlink(&target_path, &link_path)
                .with_context(|| format!("Failed to create symlink: {:?} -> {}", link_path, target_path))?;
        }

        #[cfg(not(target_os = "linux"))]
        {
            // On non-Unix platforms, we'd use different linking strategies
            // For now, we'll skip the symlink
            tracing::warn!("Symlinks not supported on this platform");
        }

        // Store link ID reference
        let link_ref_path = layer_path.join("link");
        let mut file = File::create(&link_ref_path)
            .with_context(|| format!("Failed to create link file: {:?}", link_ref_path))?;
        writeln!(file, "{}", link_id)?;

        tracing::debug!("Added layer {} with link ID {}", layer_id, link_id);

        Ok(Layer {
            id: layer_id.to_string(),
            diff_path,
            link_id,
            size: data.len() as u64,
            lower: None,
            cache_id: None,
        })
    }

    /// Get a layer by ID.
    pub fn get_layer(&self, layer_id: &str) -> Option<Layer> {
        let layer_path = self.root.join(layer_id);

        if !layer_path.exists() {
            return None;
        }

        let link_id = self.read_link_id(layer_id)?;
        let diff_path = layer_path.join(DIFF_DIR);

        // Calculate size
        let size = self.dir_size(&diff_path).unwrap_or(0);

        Some(Layer {
            id: layer_id.to_string(),
            diff_path,
            link_id,
            size,
            lower: None,
            cache_id: None,
        })
    }

    /// Create a container overlay for mounting.
    ///
    /// Sets up the merged, upper, and work directories for a container.
    pub fn create_container_overlay(
        &self,
        container_id: &str,
        lower_layers: Vec<String>,
    ) -> Result<ContainerOverlay> {
        let container_dir = self.root.join(container_id);

        let merged = container_dir.join("merged");
        let upper = container_dir.join("upper");
        let work = container_dir.join("work");

        // Create directories
        create_dir_all(&merged)
            .with_context(|| format!("Failed to create merged directory: {:?}", merged))?;
        create_dir_all(&upper)
            .with_context(|| format!("Failed to create upper directory: {:?}", upper))?;
        create_dir_all(&work)
            .with_context(|| format!("Failed to create work directory: {:?}", work))?;

        let overlay = ContainerOverlay {
            container_id: container_id.to_string(),
            merged,
            upper,
            work,
            lowers: lower_layers,
        };

        tracing::debug!("Created container overlay for {}", container_id);

        Ok(overlay)
    }

    /// Mount overlayfs for a container.
    #[cfg(target_os = "linux")]
    pub fn mount(&self, overlay: &ContainerOverlay) -> Result<()> {
        use nix::mount::{mount, MsFlags};

        // Build lower directory string
        // Lower layers are specified with the first layer being the topmost
        let lower_str = overlay.lowers.iter()
            .map(|id| {
                if let Some(link_id) = self.read_link_id(id) {
                    self.layer_diff_path_by_link(&link_id)
                } else {
                    self.layer_diff_path(id)
                }
                .to_string_lossy()
                .to_string()
            })
            .collect::<Vec<_>>()
            .join(":");

        let options = format!(
            "lowerdir={},upperdir={},workdir={}",
            lower_str,
            overlay.upper.display(),
            overlay.work.display()
        );

        // Mount overlayfs
        let options_cstr = std::ffi::CString::new(options.as_str())
            .context("Invalid mount options")?;
        mount(
            Some("overlay"),
            overlay.merged.as_path(),
            Some("overlay"),
            MsFlags::MS_NOATIME,
            Some(options_cstr.as_c_str()),
        ).context("Failed to mount overlayfs")?;

        // Track the mount
        let mount_info = MountInfo {
            id: overlay.container_id.clone(),
            target: overlay.merged.clone(),
            lowers: overlay.lowers.clone(),
            upper: Some(overlay.upper.clone()),
            work: overlay.work.clone(),
        };

        let mut mounts = self.mounts.write()
            .expect("mounts lock poisoned");
        mounts.push(mount_info);

        tracing::info!("Mounted overlayfs for container {} at {:?}", overlay.container_id, overlay.merged);

        Ok(())
    }

    /// Unmount overlayfs for a container.
    #[cfg(target_os = "linux")]
    pub fn unmount(&self, container_id: &str) -> Result<()> {
        use nix::mount::umount;

        // Find and remove mount info
        let mount_info = {
            let mut mounts = self.mounts.write()
                .expect("mounts lock poisoned");
            mounts.iter()
                .position(|m| m.id == container_id)
                .map(|pos| mounts.remove(pos))
        };

        if let Some(info) = mount_info {
            umount(&info.target)
                .with_context(|| format!("Failed to unmount {:?}", info.target))?;

            tracing::info!("Unmounted overlayfs for container {}", container_id);

            Ok(())
        } else {
            Err(anyhow::anyhow!("No mount found for container {}", container_id))
        }
    }

    /// Clean up a container's overlay directories.
    pub fn remove_container_overlay(&self, container_id: &str) -> Result<()> {
        let container_dir = self.root.join(container_id);

        if container_dir.exists() {
            remove_dir_all(&container_dir)
                .with_context(|| format!("Failed to remove container directory: {:?}", container_dir))?;

            tracing::debug!("Removed container overlay for {}", container_id);
        }

        Ok(())
    }

    /// Remove a layer from storage.
    pub fn remove_layer(&self, layer_id: &str) -> Result<()> {
        let layer_path = self.root.join(layer_id);

        if !layer_path.exists() {
            return Ok(());
        }

        // Remove short link
        if let Some(link_id) = self.read_link_id(layer_id) {
            let link_path = self.layer_link_path(&link_id);
            let _ = std::fs::remove_file(&link_path);
        }

        // Remove layer directory
        remove_dir_all(&layer_path)
            .with_context(|| format!("Failed to remove layer directory: {:?}", layer_path))?;

        tracing::debug!("Removed layer {}", layer_id);

        Ok(())
    }

    /// List all layers.
    pub fn list_layers(&self) -> Result<Vec<Layer>> {
        let mut layers = Vec::new();

        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();

            // Skip non-directories and special directories
            if !path.is_dir() {
                continue;
            }

            let name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // Skip special directories
            if name == DIFF_DIR || name == LINK_DIR {
                continue;
            }

            // Try to read as layer
            if let Some(link_id) = self.read_link_id(name) {
                let diff_path = path.join(DIFF_DIR);
                let size = self.dir_size(&diff_path).unwrap_or(0);

                layers.push(Layer {
                    id: name.to_string(),
                    diff_path,
                    link_id,
                    size,
                    lower: None,
                    cache_id: None,
                });
            }
        }

        Ok(layers)
    }

    /// Generate a unique short link ID for a layer.
    fn generate_link_id(&self, layer_id: &str) -> Result<String> {
        // Use first N characters of layer ID
        let base_id = &layer_id[..MIN_LINK_ID_LENGTH.min(layer_id.len())];

        // Check for collision and extend if needed
        let mut link_id = base_id.to_string();
        let mut length = MIN_LINK_ID_LENGTH;

        while self.layer_link_path(&link_id).exists() {
            length = (length + 1).min(MAX_LINK_ID_LENGTH);
            if length > layer_id.len() {
                // Use full ID with suffix
                link_id = format!("{}_{}", layer_id, Uuid::new_v4().to_string()[..8].to_string());
                break;
            }
            link_id = layer_id[..length].to_string();
        }

        Ok(link_id)
    }

    /// Read the short link ID for a layer.
    fn read_link_id(&self, layer_id: &str) -> Option<String> {
        let link_ref_path = self.root.join(layer_id).join("link");

        if link_ref_path.exists() {
            fs::read_to_string(&link_ref_path).ok().map(|s| s.trim().to_string())
        } else {
            None
        }
    }

    /// Get the target of a short link.
    #[cfg(target_os = "linux")]
    fn read_link_target(&self, link_id: &str) -> Option<String> {
        let link_path = self.layer_link_path(link_id);

        if link_path.exists() {
            fs::read_link(&link_path).ok().and_then(|p| {
                p.to_str().map(|s| s.to_string())
            })
        } else {
            None
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn read_link_target(&self, _link_id: &str) -> Option<String> {
        None
    }

    /// Get layer diff path by short link ID.
    fn layer_diff_path_by_link(&self, link_id: &str) -> PathBuf {
        let link_path = self.layer_link_path(link_id);

        // Read the symlink target
        if let Ok(target) = fs::read_link(&link_path) {
            // target is "../../<layer-id>"
            if let Some(layer_id) = target.components().last() {
                return self.root.join(layer_id).join(DIFF_DIR);
            }
        }

        // Fallback
        self.root.join(link_id).join(DIFF_DIR)
    }

    /// Calculate directory size recursively.
    fn dir_size(&self, path: &Path) -> Result<u64> {
        let mut total = 0u64;

        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let entry_path = entry.path();

                if entry_path.is_dir() {
                    total += self.dir_size(&entry_path)?;
                } else {
                    total += entry.metadata()?.len();
                }
            }
        } else {
            total += fs::metadata(path)?.len();
        }

        Ok(total)
    }

    /// Get total storage usage.
    pub fn total_usage(&self) -> Result<u64> {
        self.dir_size(&self.root)
    }

    /// Clean up unused layers (garbage collection).
    pub fn gc(&self, used_layers: &[String]) -> Result<Vec<String>> {
        let all_layers = self.list_layers()?;
        let used_set: std::collections::HashSet<_> = used_layers.iter().collect();

        let mut removed = Vec::new();

        for layer in all_layers {
            if !used_set.contains(&layer.id) {
                self.remove_layer(&layer.id)?;
                removed.push(layer.id);
            }
        }

        tracing::info!("Garbage collected {} layers", removed.len());

        Ok(removed)
    }
}

impl Default for OverlayfsDriver {
    fn default() -> Self {
        Self::new().expect("Failed to create overlayfs driver")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlayfs_driver_new() {
        let driver = OverlayfsDriver::with_root(
            std::env::temp_dir().join("overlay2_test")
        ).unwrap();

        assert!(driver.root().exists());
        assert!(driver.root().join(DIFF_DIR).exists());
        assert!(driver.root().join(LINK_DIR).exists());
    }

    #[test]
    fn test_add_and_get_layer() {
        let driver = OverlayfsDriver::with_root(
            std::env::temp_dir().join("overlay2_test2")
        ).unwrap();

        let layer_id = "sha256:abcdef1234567890";
        let data = b"test layer data";

        let layer = driver.add_layer(layer_id, data).unwrap();

        assert_eq!(layer.id, layer_id);
        assert!(layer.diff_path.exists());
        assert!(!layer.link_id.is_empty());

        let retrieved = driver.get_layer(layer_id).unwrap();
        assert_eq!(retrieved.id, layer_id);
        assert_eq!(retrieved.link_id, layer.link_id);
    }

    #[test]
    fn test_create_container_overlay() {
        let driver = OverlayfsDriver::with_root(
            std::env::temp_dir().join("overlay2_test3")
        ).unwrap();

        let overlay = driver.create_container_overlay(
            "test-container",
            vec!["layer1".to_string(), "layer2".to_string()]
        ).unwrap();

        assert!(overlay.merged.exists());
        assert!(overlay.upper.exists());
        assert!(overlay.work.exists());
        assert_eq!(overlay.container_id, "test-container");
        assert_eq!(overlay.lowers.len(), 2);
    }
}
