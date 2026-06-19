//! Manifest integrity checks performed on pull (E7).
//!
//! Two classes of defense against a malicious or buggy registry:
//!   1. **Resource limits** — refuse a manifest with an absurd layer count or
//!      total size before downloading anything (OOM / disk-exhaustion DoS).
//!   2. **Consistency** — verify the config blob actually matches the manifest
//!      that referenced it (digest + layer/diff_id count), so a tampered or
//!      mismatched config can't slip through.

use anyhow::{Context, Result};
use oci_spec::image::ImageManifest;

/// Caps applied to a manifest before its layers are fetched.
#[derive(Debug, Clone)]
pub struct ManifestLimits {
    pub max_layers: usize,
    pub max_total_bytes: u64,
}

impl Default for ManifestLimits {
    fn default() -> Self {
        Self {
            max_layers: 1024,
            max_total_bytes: 50 * 1024 * 1024 * 1024, // 50 GiB compressed
        }
    }
}

impl ManifestLimits {
    /// Defaults, overridable via `EXO_MAX_LAYERS` / `EXO_MAX_IMAGE_BYTES`.
    pub fn from_env() -> Self {
        let mut l = Self::default();
        if let Some(v) = std::env::var("EXO_MAX_LAYERS").ok().and_then(|v| v.parse().ok()) {
            l.max_layers = v;
        }
        if let Some(v) = std::env::var("EXO_MAX_IMAGE_BYTES").ok().and_then(|v| v.parse().ok()) {
            l.max_total_bytes = v;
        }
        l
    }

    /// Reject manifests that exceed the layer-count or total-size caps.
    pub fn check(&self, manifest: &ImageManifest) -> Result<()> {
        let layers = manifest.layers();
        if layers.len() > self.max_layers {
            anyhow::bail!(
                "manifest has {} layers, exceeding the limit of {} (possible DoS)",
                layers.len(),
                self.max_layers
            );
        }
        let total: u64 = layers.iter().map(|l| l.size()).sum();
        if total > self.max_total_bytes {
            anyhow::bail!(
                "manifest total layer size {} bytes exceeds the limit of {} (possible DoS)",
                total,
                self.max_total_bytes
            );
        }
        Ok(())
    }
}

/// Verify the downloaded config blob is the one the manifest references, and
/// that it describes the same number of layers.
pub fn verify_config_consistency(manifest: &ImageManifest, config_bytes: &[u8]) -> Result<()> {
    use sha2::{Digest as _, Sha256};

    let expected = manifest.config().digest().to_string();
    let actual = format!("sha256:{:x}", Sha256::digest(config_bytes));
    if actual != expected {
        anyhow::bail!("config digest mismatch: manifest says {expected}, blob is {actual}");
    }

    // Layer count must match the config's rootfs.diff_ids.
    let config: oci_spec::image::ImageConfiguration =
        serde_json::from_slice(config_bytes).context("parsing image config")?;
    let diff_ids = config.rootfs().diff_ids().len();
    let layers = manifest.layers().len();
    if diff_ids != layers {
        anyhow::bail!(
            "manifest/config mismatch: {layers} layers but {diff_ids} diff_ids"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_spec::image::{
        ConfigBuilder, DescriptorBuilder, ImageConfigurationBuilder, ImageManifestBuilder,
        MediaType, RootFsBuilder, Sha256Digest, SCHEMA_VERSION,
    };

    fn layer_desc(size: u64) -> oci_spec::image::Descriptor {
        DescriptorBuilder::default()
            .media_type(MediaType::ImageLayerGzip)
            .size(size)
            .digest("0000000000000000000000000000000000000000000000000000000000000000"
                .parse::<Sha256Digest>().unwrap())
            .build()
            .unwrap()
    }

    fn manifest_with(layers: usize, each: u64) -> ImageManifest {
        let config = DescriptorBuilder::default()
            .media_type(MediaType::ImageConfig)
            .size(1u64)
            .digest("1111111111111111111111111111111111111111111111111111111111111111"
                .parse::<Sha256Digest>().unwrap())
            .build()
            .unwrap();
        ImageManifestBuilder::default()
            .schema_version(SCHEMA_VERSION)
            .config(config)
            .layers((0..layers).map(|_| layer_desc(each)).collect::<Vec<_>>())
            .build()
            .unwrap()
    }

    #[test]
    fn rejects_too_many_layers() {
        let limits = ManifestLimits { max_layers: 4, max_total_bytes: u64::MAX };
        assert!(limits.check(&manifest_with(3, 10)).is_ok());
        assert!(limits.check(&manifest_with(5, 10)).is_err());
    }

    #[test]
    fn rejects_oversized_total() {
        let limits = ManifestLimits { max_layers: usize::MAX, max_total_bytes: 100 };
        assert!(limits.check(&manifest_with(2, 40)).is_ok()); // 80 <= 100
        assert!(limits.check(&manifest_with(2, 60)).is_err()); // 120 > 100
    }

    #[test]
    fn config_consistency_detects_digest_and_count_mismatch() {
        // Build a manifest for 1 layer plus a matching config (1 diff_id).
        let manifest = manifest_with(1, 10);
        let config = ImageConfigurationBuilder::default()
            .config(ConfigBuilder::default().build().unwrap())
            .rootfs(
                RootFsBuilder::default()
                    .typ("layers".to_string())
                    .diff_ids(vec!["sha256:aaa".to_string()])
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let good = serde_json::to_vec(&config).unwrap();

        // Digest mismatch: manifest.config digest won't equal this blob's hash.
        assert!(verify_config_consistency(&manifest, &good).is_err());

        // Count mismatch: 2-layer manifest vs 1-diff_id config.
        let two = manifest_with(2, 10);
        assert!(verify_config_consistency(&two, &good).is_err());
    }
}
