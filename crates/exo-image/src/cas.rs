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
use tracing::{debug, trace, warn};

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
        (self.reclaimable_via_dedup() * 100)
            .checked_div(self.logical_bytes)
            .unwrap_or(0) as u32
    }
}

/// Per-layer detail for `exo image inspect`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerInfo {
    pub digest: String,
    /// Extracted size on disk (bytes).
    pub size: u64,
    /// How many images reference this layer (>1 means shared).
    pub refcount: usize,
}

/// Full inspection of a locally-stored image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub reference: String,
    pub config_digest: String,
    pub layers: Vec<LayerInfo>,
    /// Sum of this image's layer sizes (its logical footprint).
    pub total_size: u64,
    /// Bytes unique to this image (layers only it references).
    pub exclusive_size: u64,
}

/// Result of a store integrity scan (`exo system check`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreReport {
    /// Registered images.
    pub images: usize,
    /// Images referencing one or more missing layers (ref -> missing digests).
    pub dangling_images: Vec<(String, Vec<String>)>,
    /// Extracted layers no image references (reclaimable).
    pub orphan_layers: Vec<String>,
}

impl StoreReport {
    /// Whether the store is fully consistent.
    pub fn is_healthy(&self) -> bool {
        self.dangling_images.is_empty() && self.orphan_layers.is_empty()
    }
}

/// Default cap on a single layer's uncompressed size (decompression-bomb guard).
const DEFAULT_MAX_LAYER_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GiB

/// Default cap on total extracted-layer store size.
const DEFAULT_MAX_STORE_BYTES: u64 = 100 * 1024 * 1024 * 1024; // 100 GiB

/// Content-addressed layer store.
#[derive(Clone)]
pub struct LayerStore {
    root: PathBuf,
    /// Reject a layer whose declared uncompressed size exceeds this.
    max_layer_bytes: u64,
    /// Refuse to extract once the store has grown past this.
    max_store_bytes: u64,
}

impl LayerStore {
    /// Open (creating if needed) a layer store rooted under `root`.
    /// Lives alongside the existing `blobs/` and `rootfs/` dirs.
    /// Size caps can be overridden via `EXO_MAX_LAYER_BYTES` / `EXO_MAX_STORE_BYTES`.
    pub fn new(root: PathBuf) -> Self {
        std::fs::create_dir_all(root.join("layers")).ok();
        std::fs::create_dir_all(root.join("index")).ok();
        let env_u64 = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        Self {
            max_layer_bytes: env_u64("EXO_MAX_LAYER_BYTES", DEFAULT_MAX_LAYER_BYTES),
            max_store_bytes: env_u64("EXO_MAX_STORE_BYTES", DEFAULT_MAX_STORE_BYTES),
            root,
        }
    }

    /// Set the per-layer uncompressed size cap (decompression-bomb guard).
    pub fn with_max_layer_bytes(mut self, max: u64) -> Self {
        self.max_layer_bytes = max;
        self
    }

    /// Set the total store-size cap.
    pub fn with_max_store_bytes(mut self, max: u64) -> Self {
        self.max_store_bytes = max;
        self
    }

    /// Total bytes of extracted layers currently on disk.
    pub fn store_physical_bytes(&self) -> u64 {
        let mut total = 0;
        if let Ok(entries) = std::fs::read_dir(self.root.join("layers")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with(".tmp-") && entry.path().is_dir() {
                    total += dir_size(&entry.path());
                }
            }
        }
        total
    }

    /// Directory holding the extracted content of one layer.
    pub fn layer_dir(&self, digest: &str) -> PathBuf {
        self.root.join("layers").join(sanitize(digest))
    }

    /// Whether a layer has already been extracted.
    pub fn has_layer(&self, digest: &str) -> bool {
        self.layer_dir(digest).join(".exo-layer-ok").exists()
    }

    /// Extract a layer tarball (gzip- or zstd-aware) into the store, exactly once.
    /// No-op if the layer is already present (the dedup fast path).
    ///
    /// `media_type` is used as a hint when the blob magic is ambiguous; it is not
    /// fully trusted because registries can mislabel blobs.
    ///
    /// Hardening: the blob's sha256 is verified against `digest` before any
    /// extraction, so a corrupted or tampered layer can never reach the store
    /// (supply-chain integrity). Tar entries are also confined to the layer dir
    /// (no path-traversal escape) by `unpack_in`.
    pub fn extract_layer(
        &self,
        blob_path: &Path,
        digest: &str,
        media_type: Option<&str>,
    ) -> Result<()> {
        if self.has_layer(digest) {
            trace!("layer {} already extracted — dedup hit", digest);
            return Ok(());
        }
        // Hard store-size quota. We don't auto-evict here: an in-flight pull's
        // own layers aren't registered yet, so pruning mid-pull could delete
        // them. Bail with guidance instead.
        if self.store_physical_bytes() >= self.max_store_bytes {
            anyhow::bail!(
                "image store is at its size limit ({} bytes); run `exo system prune` \
                 or raise EXO_MAX_STORE_BYTES",
                self.max_store_bytes
            );
        }
        verify_blob_digest(blob_path, digest)
            .with_context(|| format!("integrity check failed for layer {}", digest))?;
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
        // dir so cross-layer whiteouts can be applied at compose time. The size
        // cap rejects decompression bombs before they fill the disk.
        extract_raw(blob_path, &tmp, self.max_layer_bytes)
            .with_context(|| format!("extracting layer {}", digest))?;

        if dest.exists() {
            std::fs::remove_dir_all(&dest).ok();
        }
        std::fs::rename(&tmp, &dest)
            .with_context(|| format!("committing layer {} to store", digest))?;
        std::fs::write(dest.join(".exo-layer-ok"), b"1")?;
        debug!("extracted layer {} into CAS (media_type={:?})", digest, media_type);
        Ok(())
    }

    /// Commit a directory tree as a new layer (E2 build primitive).
    ///
    /// Tars + gzips `content_dir`, content-addresses it by the compressed blob's
    /// sha256, writes the blob into `blobs/`, and extracts it into the CAS so it's
    /// immediately usable in a composed rootfs. Returns the new layer's digest.
    /// Idempotent: identical content yields the same digest and is stored once.
    pub fn commit_layer(&self, content_dir: &Path) -> Result<String> {
        use sha2::{Digest as _, Sha256};

        // Build the gzipped tar in memory (build layers are small: COPY deltas).
        let mut blob = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut blob, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            tar.follow_symlinks(false);
            tar.append_dir_all(".", content_dir)
                .with_context(|| format!("taring {:?}", content_dir))?;
            tar.into_inner()?.finish()?;
        }

        let digest = format!("sha256:{:x}", Sha256::digest(&blob));

        // Persist the blob next to pulled layers, then extract into the CAS.
        let blob_path = self.root.join("blobs").join(sanitize(&digest));
        std::fs::create_dir_all(self.root.join("blobs")).ok();
        std::fs::write(&blob_path, &blob)
            .with_context(|| format!("writing built layer blob {:?}", blob_path))?;
        self.extract_layer(&blob_path, &digest, None)
            .with_context(|| format!("extracting committed layer {}", digest))?;
        debug!("committed built layer {}", digest);
        Ok(digest)
    }

    // --- index ------------------------------------------------------------

    fn index_path(&self) -> PathBuf {
        self.root.join("index").join("images.json")
    }

    /// Run `f` while holding an exclusive lock on the index, so concurrent
    /// `pull`/`build`/`rmi`/`prune` from multiple processes can't lose updates
    /// (each does load -> mutate -> save, which races without this). The lock is
    /// a `create_new` lockfile; a lock older than 30s is treated as stale (from a
    /// crashed process) and stolen, so a crash can't wedge the store forever.
    fn with_lock<T>(&self, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let lock = self.root.join("index").join(".lock");
        std::fs::create_dir_all(self.root.join("index")).ok();
        let start = std::time::Instant::now();
        loop {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
                Ok(_) => break,
                // Any failure is treated as transient and retried until timeout.
                // Besides AlreadyExists (lock held), Windows can return
                // PermissionDenied (delete-pending while the holder removes the
                // lock) or a sharing violation (AV/indexer) — none are fatal.
                Err(_) => {
                    // Steal a stale lock left by a crashed process.
                    if let Ok(meta) = std::fs::metadata(&lock) {
                        if meta.modified().ok()
                            .and_then(|m| m.elapsed().ok())
                            .map(|d| d.as_secs() >= 30)
                            .unwrap_or(false)
                        {
                            std::fs::remove_file(&lock).ok();
                            continue;
                        }
                    }
                    if start.elapsed().as_secs() >= 30 {
                        anyhow::bail!("timed out waiting for image index lock");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
        let result = f(self);
        std::fs::remove_file(&lock).ok();
        result
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
        self.with_lock(|s| {
            let mut index = s.load_index()?;
            index.images.insert(
                reference.to_string(),
                ImageRecord { layers, config_digest },
            );
            s.save_index(&index)
        })
    }

    /// Remove an image from the index. Does not delete layers (use `prune`).
    pub fn unregister_image(&self, reference: &str) -> Result<bool> {
        self.with_lock(|s| {
            let mut index = s.load_index()?;
            let removed = index.images.remove(reference).is_some();
            if removed {
                s.save_index(&index)?;
            }
            Ok(removed)
        })
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
    /// Returns (layers_removed, bytes_reclaimed). Holds the index lock so it
    /// can't race a concurrent register into deleting a just-referenced layer.
    pub fn prune(&self) -> Result<(usize, u64)> {
        self.with_lock(|s| s.prune_locked())
    }

    fn prune_locked(&self) -> Result<(usize, u64)> {
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

    /// Check whether an extracted layer contains any OCI whiteout markers.
    /// Layers with whiteouts cannot be used directly as overlay lowerdirs because
    /// overlayfs only interprets whiteouts in the upperdir, not in lower layers.
    pub fn layer_has_whiteouts(&self, digest: &str) -> bool {
        self.layer_dir(digest)
            .join(".wh..wh..opq")
            .exists()
            || self.dir_has_wh_entries(&self.layer_dir(digest))
    }

    fn dir_has_wh_entries(&self, dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false; };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(".wh.") {
                return true;
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && self.dir_has_wh_entries(&entry.path())
            {
                return true;
            }
        }
        false
    }

    /// Return the layer directories for `image_ref` as an ordered list suitable
    /// for use as overlay `lowerdir`s (highest layer first), but only if every
    /// layer is whiteout-free. If any layer contains whiteout markers we fall
    /// back to `compose_rootfs` so the markers are applied correctly.
    pub fn try_overlay_lowerdirs(&self,
        image_ref: &str,
    ) -> Option<Vec<PathBuf>> {
        let index = self.load_index().ok()?;
        let record = index.images.get(image_ref)?;
        let mut lowerdirs = Vec::with_capacity(record.layers.len());
        for digest in record.layers.iter().rev() {
            if !self.has_layer(digest) || self.layer_has_whiteouts(digest) {
                return None;
            }
            lowerdirs.push(self.layer_dir(digest));
        }
        Some(lowerdirs)
    }

    /// Inspect a registered image: its layers, their sizes, and how much disk
    /// is shared with other images vs. exclusive to this one.
    pub fn inspect(&self, reference: &str) -> Result<Option<ImageInfo>> {
        let index = self.load_index()?;
        let Some(record) = index.images.get(reference) else {
            return Ok(None);
        };

        let mut layers = Vec::new();
        let mut total_size = 0u64;
        let mut exclusive_size = 0u64;
        for digest in &record.layers {
            let size = dir_size(&self.layer_dir(digest));
            let refcount = index
                .images
                .values()
                .filter(|r| r.layers.iter().any(|l| l == digest))
                .count();
            total_size += size;
            if refcount <= 1 {
                exclusive_size += size;
            }
            layers.push(LayerInfo {
                digest: digest.clone(),
                size,
                refcount,
            });
        }

        Ok(Some(ImageInfo {
            reference: reference.to_string(),
            config_digest: record.config_digest.clone(),
            layers,
            total_size,
            exclusive_size,
        }))
    }

    /// Scan the store for inconsistencies: images whose layers are missing from
    /// the CAS, and extracted layers no image references.
    pub fn check(&self) -> Result<StoreReport> {
        let index = self.load_index()?;

        let mut dangling_images = Vec::new();
        let mut referenced = std::collections::HashSet::new();
        for (reference, rec) in &index.images {
            let mut missing = Vec::new();
            for l in &rec.layers {
                referenced.insert(sanitize(l));
                if !self.has_layer(l) {
                    missing.push(l.clone());
                }
            }
            if !missing.is_empty() {
                dangling_images.push((reference.clone(), missing));
            }
        }

        let mut orphan_layers = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.root.join("layers")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(".tmp-") || !entry.path().is_dir() {
                    continue;
                }
                if !referenced.contains(&name) {
                    orphan_layers.push(name);
                }
            }
        }

        dangling_images.sort();
        orphan_layers.sort();
        Ok(StoreReport {
            images: index.images.len(),
            dangling_images,
            orphan_layers,
        })
    }

    /// Repair the store: unregister images with missing layers (they can't be
    /// composed/run anyway) and prune orphaned layers. Returns
    /// (images_removed, layers_pruned).
    pub fn repair(&self) -> Result<(usize, usize)> {
        let report = self.check()?;
        let mut images_removed = 0;
        for (reference, _) in &report.dangling_images {
            if self.unregister_image(reference)? {
                images_removed += 1;
            }
        }
        let (layers_pruned, _) = self.prune()?;
        Ok((images_removed, layers_pruned))
    }

    /// Root path of the store.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// True if any *intermediate* component of `rel` already exists under `dest` as
/// a symlink — meaning writing `dest/rel` would follow it outside the rootfs.
/// The final component itself may legitimately be a symlink (a leaf), so it is
/// not checked here.
fn parent_has_symlink(dest: &Path, rel: &Path) -> bool {
    let comps: Vec<_> = rel.components().collect();
    let mut cur = dest.to_path_buf();
    for comp in comps.iter().take(comps.len().saturating_sub(1)) {
        cur.push(comp);
        if let Ok(meta) = std::fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                return true;
            }
        }
    }
    false
}

/// Verify a blob file's sha256 matches its `sha256:...` digest. Digests without
/// a recognized `sha256:` prefix are accepted (e.g. synthetic test digests) so
/// the store stays usable for content that isn't registry-sourced.
fn verify_blob_digest(blob_path: &Path, digest: &str) -> Result<()> {
    use sha2::{Digest as _, Sha256};
    let Some(expected) = digest.strip_prefix("sha256:") else {
        return Ok(());
    };
    let mut file = std::fs::File::open(blob_path)
        .with_context(|| format!("opening blob {:?}", blob_path))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        anyhow::bail!("digest mismatch: expected sha256:{expected}, got sha256:{actual}");
    }
    Ok(())
}

/// Raw tar extraction (gzip- and zstd-aware) that keeps every entry, including
/// `.wh.` whiteout markers, so they survive into the content-addressed layer dir.
/// Aborts if the cumulative declared size exceeds `max_bytes` (bomb guard).
fn extract_raw(layer_tar: &Path, dest: &Path, max_bytes: u64) -> Result<()> {
    use std::io::Read;
    let mut head = std::fs::File::open(layer_tar)?;
    let mut magic = [0u8; 4];
    let n = head.read(&mut magic)?;
    let gz = n >= 2 && magic[0] == 0x1f && magic[1] == 0x8b;
    let zst = n >= 4 && magic == [0x28, 0xb5, 0x2f, 0xfd];

    let file = std::fs::File::open(layer_tar)?;
    let reader: Box<dyn std::io::Read> = if gz {
        Box::new(flate2::read::GzDecoder::new(file))
    } else if zst {
        Box::new(zstd::stream::read::Decoder::new(file)?)
    } else {
        Box::new(file)
    };
    let mut archive = tar::Archive::new(reader);
    archive.set_overwrite(true);
    let mut total: u64 = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        total = total.saturating_add(entry.header().size().unwrap_or(0));
        if total > max_bytes {
            anyhow::bail!(
                "layer exceeds max uncompressed size ({} bytes); possible decompression bomb",
                max_bytes
            );
        }
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

        // Symlink-escape guard: if an existing parent component under dest is a
        // symlink, writing/removing through it would follow the link outside the
        // rootfs (a lower layer planting `data -> /host` then a higher layer
        // touching `data/x`). Refuse such entries — including whiteouts, so a
        // crafted layer can't delete host files either.
        if parent_has_symlink(dest, rel) {
            warn!("skipping layer entry {:?}: parent traverses a symlink (escape guard)", rel);
            continue;
        }

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
            // If a lower layer left a symlink at this path, a higher real dir
            // overrides it (OCI semantics) — and removing it means we don't
            // create/write through the link.
            if let Ok(m) = std::fs::symlink_metadata(&target) {
                if m.file_type().is_symlink() {
                    std::fs::remove_file(&target).ok();
                }
            }
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
            // Hardlink = shared inode = the dedup win. Fall back to copy for
            // symlinks (not portably hardlinkable) or when the link fails
            // (e.g. EXDEV across filesystems).
            if ft.is_symlink() || std::fs::hard_link(&path, &target).is_err() {
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

    /// Build a zstd-compressed tar with the given (path, contents) files.
    fn make_layer_zstd(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut enc = zstd::stream::write::Encoder::new(&mut buf,
                zstd::DEFAULT_COMPRESSION_LEVEL,
            )
            .unwrap();
            let mut tar = tar::Builder::new(&mut enc);
            for (name, data) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, name, *data).unwrap();
            }
            tar.into_inner().unwrap();
            enc.finish().unwrap();
        }
        buf
    }

    /// Write a layer blob and return (path, its real `sha256:` digest), so the
    /// store's integrity check accepts it just as it would a registry blob.
    fn mklayer(dir: &Path, name: &str, files: &[(&str, &[u8])]) -> (PathBuf, String) {
        use sha2::{Digest as _, Sha256};
        let bytes = make_layer(files);
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(&bytes).unwrap();
        (p, digest)
    }

    #[test]
    fn extract_is_idempotent_and_dedups() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let (blob, d) = mklayer(tmp.path(), "l1.tar.gz", &[("bin/sh", b"shell")]);

        assert!(!store.has_layer(&d));
        store.extract_layer(&blob, &d, None).unwrap();
        assert!(store.has_layer(&d));
        // Second extract is a no-op (dedup fast path) and must not error.
        store.extract_layer(&blob, &d, None).unwrap();
        assert!(store.layer_dir(&d).join("bin/sh").exists());
    }

    #[test]
    fn extract_handles_zstd_layers() {
        use sha2::{Digest as _, Sha256};
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let bytes = make_layer_zstd(&[("bin/zstd-sh", b"zstd shell"), ("etc/motd", b"hello")]
        );
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        let blob = tmp.path().join("layer.tar.zst");
        std::fs::write(&blob, &bytes).unwrap();

        store
            .extract_layer(
                &blob,
                &digest,
                Some("application/vnd.oci.image.layer.v1.tar+zstd"),
            )
            .unwrap();

        assert!(store.has_layer(&digest));
        let layer_dir = store.layer_dir(&digest);
        assert!(layer_dir.join("bin/zstd-sh").exists());
        assert_eq!(
            std::fs::read_to_string(layer_dir.join("bin/zstd-sh")).unwrap(),
            "zstd shell"
        );
        assert!(layer_dir.join("etc/motd").exists());
    }

    #[test]
    fn extract_rejects_decompression_bomb() {
        let tmp = tempdir().unwrap();
        // Tiny cap: a layer with a 1 KiB file should be rejected.
        let store = LayerStore::new(tmp.path().to_path_buf()).with_max_layer_bytes(64);
        let (blob, d) = mklayer(tmp.path(), "big.tgz", &[("f", &[0u8; 1024])]);
        let err = store.extract_layer(&blob, &d, None).unwrap_err();
        assert!(err.to_string().contains("decompression bomb") ||
                err.root_cause().to_string().contains("max uncompressed"));
        assert!(!store.has_layer(&d));
    }

    #[test]
    fn extract_enforces_store_quota() {
        let tmp = tempdir().unwrap();
        // First layer lands fine under a generous cap.
        let store = LayerStore::new(tmp.path().to_path_buf());
        let (a, da) = mklayer(tmp.path(), "a.tgz", &[("f", &[0u8; 4096])]);
        store.extract_layer(&a, &da, None).unwrap();
        assert!(store.store_physical_bytes() > 0);

        // A second store over the same root with a 1-byte cap refuses new layers.
        let capped = LayerStore::new(tmp.path().to_path_buf()).with_max_store_bytes(1);
        let (b, db) = mklayer(tmp.path(), "b.tgz", &[("g", &[0u8; 4096])]);
        let err = capped.extract_layer(&b, &db, None).unwrap_err();
        assert!(err.to_string().contains("size limit"));
        assert!(!capped.has_layer(&db));
        // Already-present layers still succeed (dedup fast-path skips the quota).
        capped.extract_layer(&a, &da, None).unwrap();
    }

    #[test]
    fn extract_rejects_tampered_blob() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let (blob, _real) = mklayer(tmp.path(), "l.tgz", &[("f", b"x")]);
        // Claim a digest that doesn't match the bytes -> integrity check fails.
        let wrong = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let err = store.extract_layer(&blob, wrong, None).unwrap_err();
        assert!(err.to_string().contains("integrity check failed"));
        assert!(!store.has_layer(wrong));
    }

    #[test]
    fn df_reports_dedup_savings_for_shared_layers() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());

        // Shared base layer + two distinct top layers.
        let (base, db) = mklayer(tmp.path(), "base.tgz", &[("lib/libc", &[7u8; 4096])]);
        let (topa, da) = mklayer(tmp.path(), "a.tgz", &[("app/a", &[1u8; 1024])]);
        let (topb, dbb) = mklayer(tmp.path(), "b.tgz", &[("app/b", &[2u8; 1024])]);
        store.extract_layer(&base, &db, None).unwrap();
        store.extract_layer(&topa, &da, None).unwrap();
        store.extract_layer(&topb, &dbb, None).unwrap();

        store.register_image("imgA", vec![db.clone(), da], "c1".into()).unwrap();
        store.register_image("imgB", vec![db.clone(), dbb], "c2".into()).unwrap();

        let df = store.disk_usage().unwrap();
        assert_eq!(df.images, 2);
        assert_eq!(df.unique_layers, 3);
        // Logical counts the shared base twice; physical counts it once.
        assert!(df.logical_bytes > df.physical_bytes);
        assert_eq!(df.reclaimable_via_dedup(), df.logical_bytes - df.physical_bytes);
        assert_eq!(store.refcount(&db).unwrap(), 2);
    }

    #[test]
    fn inspect_separates_shared_from_exclusive() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let (base, db) = mklayer(tmp.path(), "base.tgz", &[("lib", &[7u8; 2048])]);
        let (topa, da) = mklayer(tmp.path(), "a.tgz", &[("a", &[1u8; 1024])]);
        store.extract_layer(&base, &db, None).unwrap();
        store.extract_layer(&topa, &da, None).unwrap();
        store.register_image("imgA", vec![db.clone(), da], "c1".into()).unwrap();
        store.register_image("imgB", vec![db], "c2".into()).unwrap();

        let info = store.inspect("imgA").unwrap().unwrap();
        assert_eq!(info.layers.len(), 2);
        // base is shared (refcount 2), a is exclusive (refcount 1).
        assert!(info.exclusive_size < info.total_size);
        assert!(store.inspect("nope").unwrap().is_none());
    }

    #[test]
    fn check_and_repair_fix_dangling_refs() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let (base, db) = mklayer(tmp.path(), "base.tgz", &[("f", b"x")]);
        store.extract_layer(&base, &db, None).unwrap();

        // Healthy image, plus one referencing a layer that was never extracted.
        store.register_image("good", vec![db.clone()], "c".into()).unwrap();
        store.register_image("bad", vec![db.clone(), "sha256:missing".into()], "c".into()).unwrap();

        let report = store.check().unwrap();
        assert!(!report.is_healthy());
        assert_eq!(report.dangling_images.len(), 1);
        assert_eq!(report.dangling_images[0].0, "bad");

        let (imgs, _) = store.repair().unwrap();
        assert_eq!(imgs, 1);
        assert!(store.check().unwrap().is_healthy());
        // The good image (and its layer) survive repair.
        assert!(store.has_layer(&db));
        assert!(store.inspect("good").unwrap().is_some());
    }

    #[test]
    fn concurrent_register_does_not_lose_updates() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        // Many threads register distinct images at once; without the index lock
        // the load->mutate->save race would drop most of them.
        let n = 16;
        std::thread::scope(|scope| {
            for i in 0..n {
                let s = store.clone();
                scope.spawn(move || {
                    s.register_image(&format!("img{i}"), vec![format!("sha256:l{i}")], "c".into())
                        .unwrap();
                });
            }
        });
        let index = store.load_index().unwrap();
        assert_eq!(index.images.len(), n, "lost an update under concurrency");
    }

    #[test]
    fn prune_removes_only_unreferenced_layers() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let (base, db) = mklayer(tmp.path(), "base.tgz", &[("f", b"x")]);
        let (orphan, dorph) = mklayer(tmp.path(), "o.tgz", &[("g", b"y")]);
        store.extract_layer(&base, &db, None).unwrap();
        store.extract_layer(&orphan, &dorph, None).unwrap();
        store.register_image("img", vec![db.clone()], "c".into()).unwrap();

        let (removed, _) = store.prune().unwrap();
        assert_eq!(removed, 1);
        assert!(store.has_layer(&db));
        assert!(!store.has_layer(&dorph));
    }

    #[test]
    fn commit_layer_is_content_addressed_and_usable() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());

        // Stage a "COPY" delta: files at their destination paths.
        let stage = tmp.path().join("stage");
        std::fs::create_dir_all(stage.join("app")).unwrap();
        std::fs::write(stage.join("app/main.py"), b"print('hi')").unwrap();

        let d1 = store.commit_layer(&stage).unwrap();
        assert!(store.has_layer(&d1));
        // Same content -> same digest, stored once (idempotent).
        let d2 = store.commit_layer(&stage).unwrap();
        assert_eq!(d1, d2);

        // The committed layer composes into a rootfs with the staged file.
        let rootfs = tmp.path().join("rootfs");
        store.compose_rootfs(&rootfs, &[d1]).unwrap();
        assert_eq!(std::fs::read(rootfs.join("app/main.py")).unwrap(), b"print('hi')");
    }

    #[cfg(unix)]
    #[test]
    fn compose_blocks_symlink_escape() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());

        // A host dir the attacker wants to write into.
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();

        // Hand-build two layer dirs in the CAS (bypassing tar extraction):
        // layer A plants `evil -> /abs/outside`; layer B writes `evil/pwned`.
        let mark = |d: &str| std::fs::write(store.layer_dir(d).join(".exo-layer-ok"), b"1").unwrap();
        std::fs::create_dir_all(store.layer_dir("a")).unwrap();
        std::os::unix::fs::symlink(&outside, store.layer_dir("a").join("evil")).unwrap();
        mark("a");
        std::fs::create_dir_all(store.layer_dir("b").join("evil")).unwrap();
        std::fs::write(store.layer_dir("b").join("evil/pwned"), b"x").unwrap();
        mark("b");

        let rootfs = tmp.path().join("rootfs");
        store.compose_rootfs(&rootfs, &["a".into(), "b".into()]).unwrap();

        // The escape write must NOT have landed in the host dir.
        assert!(!outside.join("pwned").exists(), "symlink escape was not blocked");
    }

    #[test]
    fn compose_rootfs_stacks_layers_and_applies_whiteouts() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());
        let (base, db) = mklayer(tmp.path(), "base.tgz", &[("etc/keep", b"k"), ("etc/gone", b"g")]);
        // Top layer overrides `keep` and whiteouts `gone`.
        let (top, dt) = mklayer(tmp.path(), "top.tgz", &[("etc/keep", b"k2"), ("etc/.wh.gone", b"")]);
        store.extract_layer(&base, &db, None).unwrap();
        store.extract_layer(&top, &dt, None).unwrap();

        let rootfs = tmp.path().join("rootfs-img");
        store.compose_rootfs(&rootfs, &[db, dt]).unwrap();

        assert_eq!(std::fs::read(rootfs.join("etc/keep")).unwrap(), b"k2");
        assert!(!rootfs.join("etc/gone").exists());
    }

    #[test]
    fn try_overlay_lowerdirs_skips_whiteouts() {
        let tmp = tempdir().unwrap();
        let store = LayerStore::new(tmp.path().to_path_buf());

        // Base layer is clean.
        let (base, db) = mklayer(tmp.path(), "base.tgz", &[("etc/keep", b"k")]);
        // Top layer whiteouts a file — not safe for direct lowerdir use.
        let (top, dt) = mklayer(
            tmp.path(),
            "top.tgz",
            &[("etc/keep", b"k2"), ("etc/.wh.gone", b"")],
        );
        store.extract_layer(&base, &db, None).unwrap();
        store.extract_layer(&top, &dt, None).unwrap();

        // Image with a whiteout must not be exposed as overlay lowerdirs.
        store
            .register_image("img-whiteouts", vec![db.clone(), dt.clone()], "c1".into())
            .unwrap();
        assert!(store.try_overlay_lowerdirs("img-whiteouts").is_none());

        // Clean image can be exposed as overlay lowerdirs (highest first).
        store
            .register_image("img-clean", vec![db.clone()], "c2".into())
            .unwrap();
        let lowerdirs = store.try_overlay_lowerdirs("img-clean").unwrap();
        assert_eq!(lowerdirs.len(), 1);
        assert!(lowerdirs[0].ends_with(sanitize(&db)));
    }
}
