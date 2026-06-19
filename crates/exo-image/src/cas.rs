//! Content-addressed layer store (E1).
//!
//! Docker/Exo space waste comes from extracting *every layer of every image*
//! into a per-image flattened rootfs: two images sharing a base layer each keep
//! a full copy on disk. This module extracts each layer **once**, keyed by its
//! digest, under `layers/<digest>/`, and tracks which images reference which
//! layers so unreferenced layers can be GC'd.
//!
//! Per-image rootfs is then composed by **hardlinking** files out of the shared
//! layer dirs (falling back to copy across filesystems). Hardlinks share inodes,
//! so file *content* common to multiple images costs disk once — the dedup win —
//! while still presenting a plain directory tree that works rootless without an
//! overlay mount.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, trace};

/// Sanitize a digest (`sha256:abc`) into a filesystem-safe component.
fn sanitize(digest: &str) -> String {
    digest.replace(':', "_")
}

/// A single image's record in the index: the ordered layers it is built from.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageRecord {
    /// Layer digests, lowest (base) first — order matters for composition.
    pub layers: Vec<String>,
    /// Config blob digest.
    pub config_digest: String,
}

/// On-disk index mapping image reference -> the layers it uses.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageIndex {
    pub images: HashMap<String, ImageRecord>,
}

/// Disk-usage accounting for `exo system df`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskUsage {
    /// Number of distinct layers physically on disk.
    pub unique_layers: usize,
    /// Bytes actually consumed by the layer store (each layer counted once).
    pub physical_bytes: u64,
    /// Bytes that would be used if every image had its own copy of every layer.
    pub logical_bytes: u64,
    /// Number of images registered.
    pub images: usize,
}

impl DiskUsage {
    /// Bytes saved by dedup (logical - physical).
    pub fn reclaimable_via_dedup(&self) -> u64 {
        self.logical_bytes.saturating_sub(self.physical_bytes)
    }

    /// Dedup ratio as a percentage of logical size (0 if nothing stored).
    pub fn savings_pct(&self) -> u32 {
        if self.logical_bytes == 0 {
            0
        } else {
            ((self.reclaimable_via_dedup() * 100) / self.logical_bytes) as u32
        }
    }
}

/// Content-addressed layer store.
#[derive(Clone)]
pub struct LayerStore {
    root: PathBuf,
}

impl LayerStore {
    /// Open (creating if needed) a layer store rooted under `root`.
    /// Lives alongside the existing `blobs/` and `rootfs/` dirs.
    pub fn new(root: PathBuf) -> Self {
        std::fs::create_dir_all(root.join("layers")).ok();
        std::fs::create_dir_all(root.join("index")).ok();
        Self { root }
    }

    /// Directory holding the extracted content of one layer.
    pub fn layer_dir(&self, digest: &str) -> PathBuf {
        self.root.join("layers").join(sanitize(digest))
    }

    /// Whether a layer has already been extracted.
    pub fn has_layer(&self, digest: &str) -> bool {
        self.layer_dir(digest).join(".exo-layer-ok").exists()
    }

    /// Extract a layer tarball (gzip-aware) into the store, exactly once.
    /// No-op if the layer is already present (the dedup fast path).
    pub fn extract_layer(&self, blob_path: &Path, digest: &str) -> Result<()> {
        if self.has_layer(digest) {
            trace!("layer {} already extracted — dedup hit", digest);
            return Ok(());
        }
        let dest = self.layer_dir(digest);
        // Extract into a temp dir then atomically rename, so a crash mid-extract
        // never leaves a half-populated layer that looks valid.
        let tmp = self
            .root
            .join("layers")
            .join(format!(".tmp-{}", sanitize(digest)));
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp).ok();
        }
        std::fs::create_dir_all(&tmp)?;

        // Raw extraction that *preserves* `.wh.` whiteout markers — the crate's
        // whiteout-aware extractor drops them, but we need them kept in the layer
        // dir so cross-layer whiteouts can be applied at compose time.
        extract_raw(blob_path, &tmp)
            .with_context(|| format!("extracting layer {}", digest))?;

        if dest.exists() {
            std::fs::remove_dir_all(&dest).ok();
        }
        std::fs::rename(&tmp, &dest)
            .with_context(|| format!("committing layer {} to store", digest))?;
        std::fs::write(dest.join(".exo-layer-ok"), b"1")?;
        debug!("extracted layer {} into CAS", digest);
        Ok(())
    }

    // --- index ------------------------------------------------------------

    fn index_path(&self) -> PathBuf {
        self.root.join("index").join("images.json")
    }

    /// Load the image index (empty if none yet).
    pub fn load_index(&self) -> Result<ImageIndex> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(ImageIndex::default());
        }
        let bytes = std::fs::read(&path).with_context(|| format!("reading {:?}", path))?;
        Ok(serde_json::from_slice(&bytes).unwrap_or_default())
    }

    fn save_index(&self, index: &ImageIndex) -> Result<()> {
        let path = self.index_path();
        let bytes = serde_json::to_vec_pretty(index)?;
        // Atomic write.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Register (or update) an image's layer membership.
    pub fn register_image(
        &self,
        reference: &str,
        layers: Vec<String>,
        config_digest: String,
    ) -> Result<()> {
        let mut index = self.load_index()?;
        index.images.insert(
            reference.to_string(),
            ImageRecord {
                layers,
                config_digest,
            },
        );
        self.save_index(&index)
    }

    /// Remove an image from the index. Does not delete layers (use `prune`).
    pub fn unregister_image(&self, reference: &str) -> Result<bool> {
        let mut index = self.load_index()?;
        let removed = index.images.remove(reference).is_some();
        if removed {
            self.save_index(&index)?;
        }
        Ok(removed)
    }

    /// How many registered images reference a given layer.
    pub fn refcount(&self, digest: &str) -> Result<usize> {
        let index = self.load_index()?;
        Ok(index
            .images
            .values()
            .filter(|r| r.layers.iter().any(|l| l == digest))
            .count())
    }

    /// Delete extracted layers no image references anymore.
    /// Returns (layers_removed, bytes_reclaimed).
    pub fn prune(&self) -> Result<(usize, u64)> {
        let index = self.load_index()?;
        let mut referenced = std::collections::HashSet::new();
        for rec in index.images.values() {
            for l in &rec.layers {
                referenced.insert(sanitize(l));
            }
        }

        let layers_dir = self.root.join("layers");
        let mut removed = 0usize;
        let mut reclaimed = 0u64;
        if let Ok(entries) = std::fs::read_dir(&layers_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // Skip temp dirs and any still-referenced layer.
                if name.starts_with(".tmp-") || referenced.contains(&name) {
                    continue;
                }
                let path = entry.path();
                if path.is_dir() {
                    reclaimed += dir_size(&path);
                    std::fs::remove_dir_all(&path).ok();
                    removed += 1;
                    debug!("pruned unreferenced layer {}", name);
                }
            }
        }
        Ok((removed, reclaimed))
    }

    /// Compute `system df` accounting across all registered images.
    pub fn disk_usage(&self) -> Result<DiskUsage> {
        let index = self.load_index()?;

        // Physical: each distinct on-disk layer counted once.
        let mut sizes: HashMap<String, u64> = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(self.root.join("layers")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(".tmp-") {
                    continue;
                }
                if entry.path().is_dir() {
                    sizes.insert(name, dir_size(&entry.path()));
                }
            }
        }
        let physical_bytes: u64 = sizes.values().sum();

        // Logical: sum each image's layers (shared layers counted per image).
        let mut logical_bytes = 0u64;
        for rec in index.images.values() {
            for l in &rec.layers {
                logical_bytes += sizes.get(&sanitize(l)).copied().unwrap_or(0);
            }
        }

        Ok(DiskUsage {
            unique_layers: sizes.len(),
            physical_bytes,
            logical_bytes,
            images: index.images.len(),
        })
    }

    /// Compose a per-image rootfs at `dest` by stacking `layers` (base first),
    /// hardlinking files from the shared layer store (copy fallback across FS),
    /// and applying OCI whiteouts between layers.
    pub fn compose_rootfs(&self, dest: &Path, layers: &[String]) -> Result<()> {
        if dest.exists() {
            std::fs::remove_dir_all(dest).ok();
        }
        std::fs::create_dir_all(dest)?;

        for digest in layers {
            let src = self.layer_dir(digest);
            if !src.exists() {
                anyhow::bail!("layer {} not extracted; cannot compose rootfs", digest);
            }
            link_layer_onto(&src, &src, dest)?;
        }
        Ok(())
    }

    /// Root path of the store.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Raw tar extraction (gzip-aware) that keeps every entry, including `.wh.`
/// whiteout markers, so they survive into the content-addressed layer dir.
fn extract_raw(layer_tar: &Path, dest: &Path) -> Result<()> {
    use std::io::Read;
    let mut head = std::fs::File::open(layer_tar)?;
    let mut magic = [0u8; 2];
    let n = head.read(&mut magic)?;
    let gz = n >= 2 && magic[0] == 0x1f && magic[1] == 0x8b;

    let file = std::fs::File::open(layer_tar)?;
    let reader: Box<dyn std::io::Read> = if gz {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut archive = tar::Archive::new(reader);
    archive.set_overwrite(true);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if let Some(parent) = dest.join(entry.path()?).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // Best-effort: tolerate special files / perms we can't recreate rootless.
        entry.unpack_in(dest).ok();
    }
    Ok(())
}

/// Recursively merge one layer dir onto `dest`, applying whiteouts and
/// hardlinking regular files. `base` is the layer root (for relative paths).
fn link_layer_onto(base: &Path, dir: &Path, dest: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip our store marker.
        if name == ".exo-layer-ok" {
            continue;
        }

        let rel = path.strip_prefix(base).unwrap();
        let target = dest.join(rel);

        // OCI whiteouts: `.wh..wh..opq` clears the dir; `.wh.foo` deletes foo.
        if name == ".wh..wh..opq" {
            if let Some(parent) = target.parent() {
                if parent.exists() {
                    std::fs::remove_dir_all(parent).ok();
                    std::fs::create_dir_all(parent).ok();
                }
            }
            continue;
        }
        if let Some(orig) = name.strip_prefix(".wh.") {
            let victim = target.parent().unwrap().join(orig);
            if victim.is_dir() {
                std::fs::remove_dir_all(&victim).ok();
            } else {
                std::fs::remove_file(&victim).ok();
            }
            continue;
        }

        let ft = entry.file_type()?;
        if ft.is_dir() {
            std::fs::create_dir_all(&target)?;
            link_layer_onto(base, &path, dest)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // A higher layer overrides a lower one.
            if target.exists() {
                std::fs::remove_file(&target).ok();
            }
            // Hardlink = shared inode = the dedup win. Fall back to copy when
            // the destination is on a different filesystem (EXDEV) or for
            // symlinks, which can't be hardlinked portably.
            if ft.is_symlink() {
                copy_any(&path, &target)?;
            } else if std::fs::hard_link(&path, &target).is_err() {
                copy_any(&path, &target)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_any(src: &Path, dst: &Path) -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let link = std::fs::read_link(src)?;
        std::os::unix::fs::symlink(link, dst)?;
    } else if meta.file_type().is_fifo() || meta.file_type().is_socket() {
        // Skip device-like special files we can't meaningfully copy.
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn copy_any(src: &Path, dst: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        // Best effort on non-unix: copy the link target's bytes.
        std::fs::copy(src, dst)?;
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// Sum of regular-file sizes under `path` (best effort).
fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                match entry.file_type() {
                    Ok(ft) if ft.is_dir() => stack.push(entry.path()),
                    Ok(ft) if ft.is_file() => {
                        if let Ok(m) = entry.metadata() {
                            total += m.len();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /// Build a gzipped tar with the given (path, contents) files.
    fn make_layer(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            for (name, data) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, name, *data).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        buf
    }

    fn write_blob(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn extract_is_idempotent_and_dedups() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let blob = write_blob(tmp.path(), "l1.tar.gz", &make_layer(&[("bin/sh", b"shell")]));

        assert!(!store.has_layer("sha256:aaa"));
        store.extract_layer(&blob, "sha256:aaa").unwrap();
        assert!(store.has_layer("sha256:aaa"));
        // Second extract is a no-op (dedup fast path) and must not error.
        store.extract_layer(&blob, "sha256:aaa").unwrap();
        assert!(store.layer_dir("sha256:aaa").join("bin/sh").exists());
    }

    #[test]
    fn df_reports_dedup_savings_for_shared_layers() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());

        // Shared base layer + two distinct top layers.
        let base = write_blob(tmp.path(), "base.tgz", &make_layer(&[("lib/libc", &[7u8; 4096])]));
        let topa = write_blob(tmp.path(), "a.tgz", &make_layer(&[("app/a", &[1u8; 1024])]));
        let topb = write_blob(tmp.path(), "b.tgz", &make_layer(&[("app/b", &[2u8; 1024])]));
        store.extract_layer(&base, "sha256:base").unwrap();
        store.extract_layer(&topa, "sha256:a").unwrap();
        store.extract_layer(&topb, "sha256:b").unwrap();

        store
            .register_image("imgA", vec!["sha256:base".into(), "sha256:a".into()], "c1".into())
            .unwrap();
        store
            .register_image("imgB", vec!["sha256:base".into(), "sha256:b".into()], "c2".into())
            .unwrap();

        let df = store.disk_usage().unwrap();
        assert_eq!(df.images, 2);
        assert_eq!(df.unique_layers, 3);
        // Logical counts the shared base twice; physical counts it once.
        assert!(df.logical_bytes > df.physical_bytes);
        assert_eq!(df.reclaimable_via_dedup(), df.logical_bytes - df.physical_bytes);
        assert_eq!(store.refcount("sha256:base").unwrap(), 2);
    }

    #[test]
    fn prune_removes_only_unreferenced_layers() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let base = write_blob(tmp.path(), "base.tgz", &make_layer(&[("f", b"x")]));
        let orphan = write_blob(tmp.path(), "o.tgz", &make_layer(&[("g", b"y")]));
        store.extract_layer(&base, "sha256:base").unwrap();
        store.extract_layer(&orphan, "sha256:orphan").unwrap();
        store
            .register_image("img", vec!["sha256:base".into()], "c".into())
            .unwrap();

        let (removed, _) = store.prune().unwrap();
        assert_eq!(removed, 1);
        assert!(store.has_layer("sha256:base"));
        assert!(!store.has_layer("sha256:orphan"));
    }

    #[test]
    fn compose_rootfs_stacks_layers_and_applies_whiteouts() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let base = write_blob(
            tmp.path(),
            "base.tgz",
            &make_layer(&[("etc/keep", b"k"), ("etc/gone", b"g")]),
        );
        // Top layer overrides `keep` and whiteouts `gone`.
        let top = write_blob(
            tmp.path(),
            "top.tgz",
            &make_layer(&[("etc/keep", b"k2"), ("etc/.wh.gone", b"")]),
        );
        store.extract_layer(&base, "sha256:base").unwrap();
        store.extract_layer(&top, "sha256:top").unwrap();

        let rootfs = tmp.path().join("rootfs-img");
        store
            .compose_rootfs(&rootfs, &["sha256:base".into(), "sha256:top".into()])
            .unwrap();

        assert_eq!(std::fs::read(rootfs.join("etc/keep")).unwrap(), b"k2");
        assert!(!rootfs.join("etc/gone").exists());
    }
}
