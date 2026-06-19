//! Generate an OCI image config + manifest for a locally-built image (E2).
//!
//! After `exo build` commits its layers into the CAS, this turns that layer set
//! plus the manifest's ENV/CMD/workdir into a spec-compliant OCI config and
//! manifest, stored as blobs so the image can be `exo push`ed to any registry.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::Read;

use oci_spec::image::{
    Arch, ConfigBuilder, DescriptorBuilder, ImageConfigurationBuilder, ImageManifestBuilder,
    MediaType, Os, RootFsBuilder, Sha256Digest, SCHEMA_VERSION,
};

use crate::{ImageReference, ImageStore};

/// Strip the `sha256:` prefix to get the bare hex a `Sha256Digest` expects.
fn hex(digest: &str) -> Result<Sha256Digest> {
    let bare = digest.strip_prefix("sha256:").unwrap_or(digest);
    bare.parse::<Sha256Digest>()
        .with_context(|| format!("invalid sha256 digest {digest}"))
}

/// Compute a layer's uncompressed digest (OCI `diff_id`) from its gzipped blob.
fn compute_diff_id(blob_path: &std::path::Path) -> Result<String> {
    use sha2::{Digest as _, Sha256};
    let file = std::fs::File::open(blob_path)
        .with_context(|| format!("opening layer blob {blob_path:?}"))?;
    let mut dec = flate2::read::GzDecoder::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = dec.read(&mut buf).context("decompressing layer for diff_id")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Build an OCI config + manifest for `reference` from its committed layers and
/// runtime config, store both as blobs, and save the manifest. Returns the
/// config digest.
pub fn build_and_store(
    store: &ImageStore,
    reference: &ImageReference,
    layer_digests: &[String],
    env: &BTreeMap<String, String>,
    cmd: &[String],
    workdir: Option<&str>,
) -> Result<String> {
    // diff_ids are the uncompressed digests of each layer, in order.
    let mut diff_ids = Vec::with_capacity(layer_digests.len());
    for d in layer_digests {
        diff_ids.push(compute_diff_id(&store.blob_path(d))?);
    }

    // ---- config ----
    let mut config_builder = ConfigBuilder::default();
    if !env.is_empty() {
        config_builder = config_builder
            .env(env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>());
    }
    if !cmd.is_empty() {
        config_builder = config_builder.cmd(cmd.to_vec());
    }
    if let Some(wd) = workdir {
        config_builder = config_builder.working_dir(wd.to_string());
    }
    let config = config_builder.build().context("building image config")?;

    let image_config = ImageConfigurationBuilder::default()
        .architecture(Arch::Amd64)
        .os(Os::Linux)
        .config(config)
        .rootfs(
            RootFsBuilder::default()
                .typ("layers".to_string())
                .diff_ids(diff_ids)
                .build()
                .context("building rootfs")?,
        )
        .build()
        .context("building image configuration")?;

    // Serialize + content-address the config, store as a blob.
    let config_bytes = serde_json::to_vec(&image_config).context("serializing config")?;
    let config_digest = {
        use sha2::{Digest as _, Sha256};
        format!("sha256:{:x}", Sha256::digest(&config_bytes))
    };
    let config_path = store.blob_path(&config_digest);
    std::fs::create_dir_all(config_path.parent().unwrap()).ok();
    std::fs::write(&config_path, &config_bytes)
        .with_context(|| format!("writing config blob {config_path:?}"))?;

    // ---- manifest ----
    let config_desc = DescriptorBuilder::default()
        .media_type(MediaType::ImageConfig)
        .size(config_bytes.len() as u64)
        .digest(hex(&config_digest)?)
        .build()
        .context("building config descriptor")?;

    let mut layer_descs = Vec::with_capacity(layer_digests.len());
    for d in layer_digests {
        let size = std::fs::metadata(store.blob_path(d))
            .with_context(|| format!("stat layer blob {d}"))?
            .len();
        layer_descs.push(
            DescriptorBuilder::default()
                .media_type(MediaType::ImageLayerGzip)
                .size(size)
                .digest(hex(d)?)
                .build()
                .context("building layer descriptor")?,
        );
    }

    let manifest = ImageManifestBuilder::default()
        .schema_version(SCHEMA_VERSION)
        .media_type(MediaType::ImageManifest)
        .config(config_desc)
        .layers(layer_descs)
        .build()
        .context("building image manifest")?;

    store
        .save_oci_manifest(reference, &manifest)
        .context("saving built manifest")?;

    Ok(config_digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn gz_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            for (name, data) in files {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                tar.append_data(&mut h, name, *data).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        buf
    }

    #[test]
    fn builds_config_and_manifest_for_built_image() {
        use sha2::{Digest as _, Sha256};
        let tmp = tempdir().unwrap();
        let store = ImageStore::new(tmp.path().to_path_buf());

        // Stage one layer blob in the store.
        let blob = gz_tar(&[("app/main.py", b"print(1)")]);
        let digest = format!("sha256:{:x}", Sha256::digest(&blob));
        std::fs::write(store.blob_path(&digest), &blob).unwrap();

        let reference = ImageReference::parse("test-agent:latest").unwrap();
        let mut env = BTreeMap::new();
        env.insert("LOG".to_string(), "info".to_string());

        let cfg_digest = build_and_store(
            &store,
            &reference,
            &[digest.clone()],
            &env,
            &["python".to_string(), "main.py".to_string()],
            Some("/app"),
        )
        .unwrap();

        // Config blob exists and the manifest round-trips with our layer + config.
        assert!(store.blob_path(&cfg_digest).exists());
        let bytes = store.load_manifest(&reference).unwrap();
        let manifest: oci_spec::image::ImageManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(manifest.layers().len(), 1);
        assert_eq!(manifest.config().digest().to_string(), cfg_digest);
    }
}
