//! Layer extraction with whiteout handling.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, trace};

/// Layer extractor - handles OCI layer extraction with whiteouts.
pub struct LayerExtractor;

impl LayerExtractor {
    /// Extract a single layer to a rootfs directory.
    ///
    /// OCI layers use whiteout files to mark deletions:
    /// - `.wh.filename` - delete `filename`
    /// - `.wh..wh..opq` - make directory opaque (delete all existing contents)
    pub fn extract(layer_tar: &Path, dest: &Path) -> Result<()> {
        trace!("Extracting layer {:?} to {:?}", layer_tar, dest);

        let file = std::fs::File::open(layer_tar)
            .with_context(|| format!("Failed to open layer: {:?}", layer_tar))?;

        // Check if gzipped
        let reader: Box<dyn std::io::Read> =
            if layer_tar.extension().map(|e| e == "gz").unwrap_or(false) {
                Box::new(flate2::read::GzDecoder::new(file))
            } else {
                Box::new(file)
            };

        let mut archive = tar::Archive::new(reader);

        // First pass: collect whiteouts
        let mut whiteouts: Vec<std::path::PathBuf> = Vec::new();
        let mut opaque_dirs: Vec<std::path::PathBuf> = Vec::new();

        for entry in archive.entries()? {
            let entry = entry?;
            let path = entry.path()?.to_path_buf();
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            if file_name.starts_with(".wh.") {
                if file_name == ".wh..wh..opq" {
                    opaque_dirs.push(path.parent().unwrap().to_path_buf());
                } else {
                    let original = file_name.strip_prefix(".wh.").unwrap();
                    whiteouts.push(path.parent().unwrap().join(original));
                }
            }
        }

        // Handle opaque directories
        for opaque_dir in &opaque_dirs {
            let full_path = dest.join(opaque_dir);
            if full_path.exists() {
                debug!("Opaque directory: clearing {:?}", full_path);
                std::fs::remove_dir_all(&full_path).ok();
                std::fs::create_dir_all(&full_path)?;
            }
        }

        // Handle whiteouts
        for whiteout in &whiteouts {
            let full_path = dest.join(whiteout);
            if full_path.exists() {
                debug!("Whiteout: removing {:?}", full_path);
                if full_path.is_dir() {
                    std::fs::remove_dir_all(&full_path).ok();
                } else {
                    std::fs::remove_file(&full_path).ok();
                }
            }
        }

        // Re-open archive for extraction
        let file = std::fs::File::open(layer_tar)?;
        let reader: Box<dyn std::io::Read> =
            if layer_tar.extension().map(|e| e == "gz").unwrap_or(false) {
                Box::new(flate2::read::GzDecoder::new(file))
            } else {
                Box::new(file)
            };

        let mut archive = tar::Archive::new(reader);

        // Second pass: extract files
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            let file_name = path
                .file_name()
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

            // Extract
            entry.unpack(&dest_path)?;
        }

        Ok(())
    }
}
