//! Minimal OCI registry client: pull an image's layers, compose the rootfs on
//! the host, and produce a tarball the guest agent can import.
//!
//! The guest has no TLS stack, so image provisioning always goes through the
//! host. Only what the microVM backend needs is implemented: anonymous/public
//! pulls with Bearer-challenge auth (covers Docker Hub and most registries),
//! manifest lists resolved to linux/arm64, gzip or plain tar layers, and
//! whiteout application.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::info;

const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json";

#[derive(Debug, Clone)]
struct ImageRef {
    registry: String,
    repo: String,
    tag: String,
}

/// Parse `name[:tag][/registry...]` forms with Docker Hub defaults:
/// `alpine` -> registry-1.docker.io/library/alpine:latest,
/// `ghcr.io/org/img:1.2` -> ghcr.io/org/img:1.2.
fn parse_image_ref(image: &str) -> Result<ImageRef> {
    let (name, tag) = match image.rsplit_once(':') {
        Some((n, t)) if !t.contains('/') => (n.to_string(), t.to_string()),
        _ => (image.to_string(), "latest".to_string()),
    };
    let mut parts = name.splitn(2, '/');
    let first = parts.next().unwrap_or("");
    let rest = parts.next();
    let (registry, repo) = match rest {
        Some(r) if first.contains('.') || first.contains(':') || first == "localhost" => {
            let reg = if first == "docker.io" {
                "registry-1.docker.io".to_string()
            } else {
                first.to_string()
            };
            (reg, r.to_string())
        }
        Some(r) => ("registry-1.docker.io".to_string(), format!("{}/{}", first, r)),
        None => (
            "registry-1.docker.io".to_string(),
            format!("library/{}", first),
        ),
    };
    if repo.is_empty() {
        anyhow::bail!("invalid image reference: {}", image);
    }
    Ok(ImageRef {
        registry,
        repo,
        tag,
    })
}

#[derive(Debug, Default)]
struct BearerAuth {
    realm: String,
    service: String,
    scope: String,
}

/// Perform a GET, following the registry's Bearer token challenge when asked.
async fn get_with_auth(
    client: &reqwest::Client,
    url: &str,
    accept: &str,
    auth: &mut Option<BearerAuth>,
) -> Result<reqwest::Response> {
    let mut request = client.get(url).header("Accept", accept);
    if auth.is_some() {
        let token = fetch_token(client, auth.as_ref().unwrap()).await?;
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    if response.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(response.error_for_status()?);
    }
    let challenge = response
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let parsed = parse_bearer_challenge(&challenge)
        .with_context(|| format!("registry auth challenge for {}: {}", url, challenge))?;
    *auth = Some(parsed);
    let token = fetch_token(client, auth.as_ref().unwrap()).await?;
    let response = client
        .get(url)
        .header("Accept", accept)
        .bearer_auth(token)
        .send()
        .await?;
    Ok(response.error_for_status()?)
}

fn parse_bearer_challenge(header: &str) -> Result<BearerAuth> {
    let rest = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow::anyhow!("unsupported auth challenge: {}", header))?;
    let mut auth = BearerAuth::default();
    for part in rest.split(',') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        let value = value.trim_matches('"');
        match key {
            "realm" => auth.realm = value.to_string(),
            "service" => auth.service = value.to_string(),
            "scope" => auth.scope = value.to_string(),
            _ => {}
        }
    }
    if auth.realm.is_empty() {
        anyhow::bail!("auth challenge has no realm: {}", header);
    }
    Ok(auth)
}

async fn fetch_token(client: &reqwest::Client, auth: &BearerAuth) -> Result<String> {
    #[derive(Deserialize)]
    struct TokenResponse {
        token: Option<String>,
        access_token: Option<String>,
    }
    let response: TokenResponse = client
        .get(&auth.realm)
        .query(&[
            ("service", auth.service.as_str()),
            ("scope", auth.scope.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    response
        .token
        .or(response.access_token)
        .ok_or_else(|| anyhow::anyhow!("token response from {} had no token", auth.realm))
}

#[derive(Deserialize)]
struct ManifestList {
    manifests: Vec<ManifestDescriptor>,
}

#[derive(Deserialize)]
struct ManifestDescriptor {
    digest: String,
    platform: Option<Platform>,
}

#[derive(Deserialize)]
struct Platform {
    architecture: String,
    os: String,
}

#[derive(Deserialize)]
struct ImageManifest {
    layers: Vec<LayerDescriptor>,
}

#[derive(Deserialize)]
struct LayerDescriptor {
    digest: String,
    #[serde(rename = "mediaType")]
    media_type: String,
    size: Option<u64>,
}

/// Pull `image` and write a gzipped tarball of the composed rootfs to `dest`.
pub async fn pull_rootfs_tar(image: &str, dest: &Path) -> Result<()> {
    let iref = parse_image_ref(image)?;
    let client = reqwest::Client::builder()
        .user_agent("exo-vm-mac/0.1")
        .build()?;
    let mut auth: Option<BearerAuth> = None;

    let manifest_url = format!(
        "https://{}/v2/{}/manifests/{}",
        iref.registry, iref.repo, iref.tag
    );
    info!("Fetching manifest for {} from {}", image, manifest_url);
    let response = get_with_auth(&client, &manifest_url, MANIFEST_ACCEPT, &mut auth).await?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = response.bytes().await?;

    let manifest: ImageManifest = if content_type.contains("manifest.list")
        || content_type.contains("image.index")
    {
        let list: ManifestList = serde_json::from_slice(&body)
            .with_context(|| format!("parse manifest list for {}", image))?;
        let descriptor = list
            .manifests
            .iter()
            .find(|m| {
                m.platform
                    .as_ref()
                    .map(|p| p.os == "linux" && p.architecture == "arm64")
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("no linux/arm64 variant for image {}", image))?;
        let by_digest = format!(
            "https://{}/v2/{}/manifests/{}",
            iref.registry, iref.repo, descriptor.digest
        );
        let response = get_with_auth(&client, &by_digest, MANIFEST_ACCEPT, &mut auth).await?;
        serde_json::from_slice(&response.bytes().await?)
            .with_context(|| format!("parse image manifest for {}", image))?
    } else {
        serde_json::from_slice(&body)
            .with_context(|| format!("parse image manifest for {}", image))?
    };

    if manifest.layers.is_empty() {
        anyhow::bail!("image {} has no layers", image);
    }

    let temp = tempfile::tempdir()?;
    let rootfs = temp.path().join("rootfs");
    std::fs::create_dir_all(&rootfs)?;

    for (index, layer) in manifest.layers.iter().enumerate() {
        if layer.media_type.contains("zstd") {
            anyhow::bail!(
                "zstd-compressed layers are not supported yet (image {}, layer {})",
                image,
                layer.digest
            );
        }
        let blob_url = format!(
            "https://{}/v2/{}/blobs/{}",
            iref.registry, iref.repo, layer.digest
        );
        info!(
            "Downloading layer {}/{} for {} ({} bytes)",
            index + 1,
            manifest.layers.len(),
            image,
            layer.size.unwrap_or(0)
        );
        let response = get_with_auth(&client, &blob_url, "*/*", &mut auth).await?;
        let bytes = response.bytes().await?;
        apply_layer(&bytes, &rootfs)
            .with_context(|| format!("apply layer {} of {}", layer.digest, image))?;
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(dest)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    // Real image rootfs trees contain dangling symlinks (e.g. /etc/alternatives
    // targets installed by later packages); store links as links instead of
    // following them, which would fail with ENOENT.
    builder.follow_symlinks(false);
    builder
        .append_dir_all(".", &rootfs)
        .with_context(|| format!("archive composed rootfs for {}", image))?;
    builder.finish()?;
    info!("Wrote composed rootfs for {} to {}", image, dest.display());
    Ok(())
}

/// Apply one layer archive to the rootfs: extract everything except whiteout
/// markers, then apply the deletions the markers encode.
fn apply_layer(bytes: &[u8], rootfs: &Path) -> Result<()> {
    let reader: Box<dyn std::io::Read> = if bytes.starts_with(&[0x1f, 0x8b]) {
        Box::new(flate2::read::GzDecoder::new(bytes))
    } else {
        Box::new(bytes)
    };
    let mut archive = tar::Archive::new(reader);
    let mut deletions: Vec<PathBuf> = Vec::new();
    let mut opaque_dirs: Vec<PathBuf> = Vec::new();
    let mut unpacked = 0usize;
    let mut skipped = 0usize;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if file_name == ".wh..wh..opq" {
            if let Some(parent) = path.parent() {
                opaque_dirs.push(parent.to_path_buf());
            }
            continue;
        }
        if let Some(target) = file_name.strip_prefix(".wh.") {
            if let Some(parent) = path.parent() {
                deletions.push(parent.join(target));
            }
            continue;
        }
        // Device nodes and FIFOs cannot be materialized without root; skip
        // them (the guest mounts devtmpfs at runtime anyway).
        let entry_type = entry.header().entry_type();
        if matches!(
            entry_type,
            tar::EntryType::Char | tar::EntryType::Block | tar::EntryType::Fifo
        ) {
            skipped += 1;
            continue;
        }
        if !entry.unpack_in(rootfs)? {
            anyhow::bail!("refusing to extract unsafe path {}", path.display());
        }
        unpacked += 1;
    }
    info!("applied layer: {} entries unpacked, {} skipped", unpacked, skipped);

    for dir in opaque_dirs {
        let target = rootfs.join(&dir);
        if let Ok(entries) = std::fs::read_dir(&target) {
            for entry in entries.flatten() {
                remove_path(&entry.path())?;
            }
        }
    }
    for path in deletions {
        remove_path(&rootfs.join(&path))?;
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_hub_shorthand() {
        let r = parse_image_ref("alpine").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repo, "library/alpine");
        assert_eq!(r.tag, "latest");

        let r = parse_image_ref("node:22-slim").unwrap();
        assert_eq!(r.repo, "library/node");
        assert_eq!(r.tag, "22-slim");

        let r = parse_image_ref("org/img:1.0").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repo, "org/img");
        assert_eq!(r.tag, "1.0");
    }

    #[test]
    fn parses_explicit_registry() {
        let r = parse_image_ref("ghcr.io/org/img:2").unwrap();
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repo, "org/img");
        assert_eq!(r.tag, "2");

        let r = parse_image_ref("docker.io/library/debian:12").unwrap();
        assert_eq!(r.registry, "registry-1.docker.io");
        assert_eq!(r.repo, "library/debian");
    }

    #[test]
    fn parses_bearer_challenges() {
        let auth = parse_bearer_challenge(
            r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/node:pull""#,
        )
        .unwrap();
        assert_eq!(auth.realm, "https://auth.docker.io/token");
        assert_eq!(auth.service, "registry.docker.io");
        assert_eq!(auth.scope, "repository:library/node:pull");
    }

    #[test]
    fn applies_layer_with_whiteouts() {
        let rootfs = tempfile::tempdir().unwrap();
        let make_layer = |files: &[(&str, &[u8])]| -> Vec<u8> {
            let mut builder = tar::Builder::new(Vec::new());
            let mut seen_dirs = std::collections::HashSet::new();
            for (path, data) in files {
                if let Some(parent) = Path::new(path).parent() {
                    if !parent.as_os_str().is_empty() && seen_dirs.insert(parent.to_path_buf()) {
                        let mut header = tar::Header::new_gnu();
                        header.set_entry_type(tar::EntryType::Directory);
                        header.set_size(0);
                        header.set_mode(0o755);
                        header.set_cksum();
                        builder
                            .append_data(&mut header, parent, std::io::empty())
                            .unwrap();
                    }
                }
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, path, *data).unwrap();
            }
            builder.into_inner().unwrap()
        };

        apply_layer(
            &make_layer(&[
                ("etc/keep.txt", b"keep"),
                ("etc/drop.txt", b"drop"),
                ("dir/stale.txt", b"stale"),
            ]),
            rootfs.path(),
        )
        .unwrap();
        apply_layer(
            &make_layer(&[("etc/.wh.drop.txt", b""), ("dir/.wh..wh..opq", b"")]),
            rootfs.path(),
        )
        .unwrap();

        assert!(rootfs.path().join("etc/keep.txt").exists());
        assert!(!rootfs.path().join("etc/drop.txt").exists());
        assert!(!rootfs.path().join("dir/stale.txt").exists());
        assert!(rootfs.path().join("dir").exists());
    }
}
