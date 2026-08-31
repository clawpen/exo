//! OCI Registry client with authentication support.
//!
//! Supports Docker Hub, GitHub Container Registry (GHCR), and other
//! OCI-compliant registries.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::{ImageReference, ImageStore};
use exo_runtime::ExoError;

/// Classify a registry HTTP failure into the typed taxonomy
/// (`docs/EXIT_CODES.md`). Pure function over the status code so it is
/// testable without network access.
///
/// - 404 → `ImageNotFound` (the manifest/blob reference does not exist)
/// - 401/403 → `RegistryAuth` (fix credentials; retrying as-is won't help)
/// - 5xx → `RegistryUnavailable` (registry-side, transient → retryable)
/// - anything else → `Internal` with the status and body preserved
fn classify_registry_status(status: u16, what: &str, detail: &str) -> ExoError {
    match status {
        404 => ExoError::ImageNotFound(what.to_string()),
        401 | 403 => ExoError::RegistryAuth(format!("{what} (HTTP {status})")),
        500..=599 => ExoError::RegistryUnavailable(format!("{what} (HTTP {status})")),
        _ => ExoError::Internal(format!(
            "registry request for {what} failed: HTTP {status} - {detail}"
        )),
    }
}

/// Map a reqwest transport failure (no HTTP response) into the taxonomy:
/// connect/timeout errors are transient → `RegistryUnavailable` (retryable).
fn map_transport_error(err: reqwest::Error, what: &str) -> anyhow::Error {
    if err.is_connect() || err.is_timeout() {
        return ExoError::RegistryUnavailable(format!("{what}: {err}")).into();
    }
    anyhow::Error::new(err).context(format!("registry request failed: {what}"))
}

/// Registry authentication credentials.
#[derive(Debug, Clone)]
pub struct RegistryAuth {
    /// Username for authentication.
    pub username: String,
    /// Password or token for authentication.
    pub password: String,
}

impl RegistryAuth {
    /// Create new authentication credentials.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Create anonymous authentication (for public repos).
    pub fn anonymous() -> Self {
        Self {
            username: String::new(),
            password: String::new(),
        }
    }

    /// Encode as Basic auth header.
    pub fn to_basic_auth(&self) -> String {
        let credentials = format!("{}:{}", self.username, self.password);
        format!("Basic {}", BASE64.encode(credentials))
    }
}

/// Docker config.json authentication entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfigAuth {
    /// Authentication string (base64 encoded username:password).
    pub auth: Option<String>,
    /// Identity token (for OAuth).
    #[serde(rename = "identitytoken")]
    pub identity_token: Option<String>,
    /// Registry token.
    #[serde(rename = "registrytoken")]
    pub registry_token: Option<String>,
}

/// Docker config.json structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Authentication entries keyed by registry URL.
    pub auths: Option<HashMap<String, DockerConfigAuth>>,
    /// Credential helpers keyed by registry.
    #[serde(rename = "credHelpers")]
    pub cred_helpers: Option<HashMap<String, String>>,
}

/// Token response from Docker auth server.
#[derive(Debug, Clone, Deserialize)]
struct TokenResponse {
    token: String,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// OCI Registry client.
pub struct RegistryClient {
    /// HTTP client.
    client: reqwest::Client,
    /// Image store for saving blobs.
    store: ImageStore,
    /// Cached authentication tokens.
    token_cache: HashMap<String, (String, std::time::Instant)>,
    /// Docker config for auth.
    docker_config: Option<DockerConfig>,
}

impl RegistryClient {
    /// Create a new registry client.
    pub fn new(store: ImageStore) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("exo-container-runtime/0.1.0")
            .build()
            .context("Failed to create HTTP client")?;

        // Load Docker config if available
        let docker_config = Self::load_docker_config()?;

        Ok(Self {
            client,
            store,
            token_cache: HashMap::new(),
            docker_config,
        })
    }

    /// Load Docker config from default locations.
    fn load_docker_config() -> Result<Option<DockerConfig>> {
        let config_paths = vec![
            dirs::home_dir().map(|h| h.join(".docker").join("config.json")),
            std::env::var("DOCKER_CONFIG")
                .ok()
                .map(|p| PathBuf::from(p).join("config.json")),
        ];

        for path in config_paths.into_iter().flatten() {
            if path.exists() {
                debug!("Loading Docker config from {:?}", path);
                match std::fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<DockerConfig>(&content) {
                        Ok(config) => {
                            debug!(
                                "Loaded Docker config with {} registries",
                                config.auths.as_ref().map(|a| a.len()).unwrap_or(0)
                            );
                            return Ok(Some(config));
                        }
                        Err(e) => {
                            warn!("Failed to parse Docker config: {}", e);
                        }
                    },
                    Err(e) => {
                        warn!("Failed to read Docker config: {}", e);
                    }
                }
            }
        }

        Ok(None)
    }

    /// Get the API URL for a registry.
    fn get_api_url(registry: &str) -> String {
        match registry {
            "docker.io" | "index.docker.io" => "https://registry-1.docker.io/v2".to_string(),
            _ => {
                format!("https://{}/v2", registry)
            }
        }
    }

    /// Get authentication for a registry from Docker config.
    fn get_auth_from_config(&self, registry: &str) -> Option<RegistryAuth> {
        let config = self.docker_config.as_ref()?;
        let auths = config.auths.as_ref()?;

        // Try various registry URL formats
        let keys_to_try = vec![
            registry.to_string(),
            format!("https://{}", registry),
            format!("https://{}/v1/", registry),
            format!("https://index.docker.io/v1/"), // For docker.io
        ];

        for key in keys_to_try {
            if let Some(auth_entry) = auths.get(&key) {
                // Try identity token first
                if let Some(ref token) = auth_entry.identity_token {
                    return Some(RegistryAuth::new("", token));
                }

                // Try registry token
                if let Some(ref token) = auth_entry.registry_token {
                    return Some(RegistryAuth::new("", token));
                }

                // Try basic auth
                if let Some(ref auth_b64) = auth_entry.auth {
                    if let Ok(decoded) = BASE64.decode(auth_b64) {
                        if let Ok(credentials) = String::from_utf8(decoded) {
                            if let Some((username, password)) = credentials.split_once(':') {
                                return Some(RegistryAuth::new(username, password));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Get an authentication token for a repository.
    async fn get_auth_token(&mut self, registry: &str, repository: &str) -> Result<String> {
        // Check cache
        let cache_key = format!("{}/{}", registry, repository);
        if let Some((token, expires)) = self.token_cache.get(&cache_key) {
            // Use token if it has more than 30 seconds left
            if expires.duration_since(std::time::Instant::now()).as_secs() > 30 {
                return Ok(token.clone());
            }
        }

        // Get credentials from config
        let auth = self.get_auth_from_config(registry);

        // For Docker Hub, use the auth service
        if registry == "docker.io" || registry == "index.docker.io" {
            let url = format!(
                "https://auth.docker.io/token?service=registry.docker.io&scope=repository:{}:pull",
                repository
            );

            let mut request = self.client.get(&url);

            // Add basic auth if we have credentials
            if let Some(ref a) = auth {
                if !a.username.is_empty() || !a.password.is_empty() {
                    request = request.header("Authorization", a.to_basic_auth());
                }
            }

            let resp = request
                .send()
                .await
                .map_err(|e| map_transport_error(e, &format!("auth token for {repository}")))?;

            if !resp.status().is_success() {
                return Err(ExoError::RegistryAuth(format!(
                    "{registry} (HTTP {})",
                    resp.status()
                ))
                .into());
            }

            let token_resp: TokenResponse = resp
                .json()
                .await
                .context("Failed to parse token response")?;

            let token = token_resp.token;
            let expires_in = token_resp.expires_in.unwrap_or(300);

            // Cache the token
            let expires_at =
                std::time::Instant::now() + std::time::Duration::from_secs(expires_in as u64);
            self.token_cache
                .insert(cache_key, (token.clone(), expires_at));

            return Ok(token);
        }

        // For GHCR, try token auth
        if registry == "ghcr.io" {
            if let Some(ref a) = auth {
                if !a.password.is_empty() {
                    return Ok(a.password.clone());
                }
            }
        }

        // For other registries, try the /v2/ endpoint for auth challenge
        let www_authenticate = self.get_auth_challenge(registry).await?;

        if let Some(realm) = www_authenticate.get("realm") {
            let mut url = realm.clone();
            url.push_str("?service=");
            if let Some(service) = www_authenticate.get("service") {
                url.push_str(&urlencoding::encode(service));
            }
            url.push_str("&scope=repository:");
            url.push_str(repository);
            url.push_str(":pull");

            let mut request = self.client.get(&url);

            if let Some(ref a) = auth {
                if !a.username.is_empty() || !a.password.is_empty() {
                    request = request.header("Authorization", a.to_basic_auth());
                }
            }

            let resp = request
                .send()
                .await
                .map_err(|e| map_transport_error(e, &format!("auth token for {repository}")))?;

            if !resp.status().is_success() {
                // Try without auth for public repos
                let resp = self
                    .client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| {
                        map_transport_error(e, &format!("anonymous auth token for {repository}"))
                    })?;

                if !resp.status().is_success() {
                    return Err(ExoError::RegistryAuth(format!(
                        "{registry} (HTTP {})",
                        resp.status()
                    ))
                    .into());
                }

                let token_resp: TokenResponse = resp
                    .json()
                    .await
                    .context("Failed to parse token response")?;

                return Ok(token_resp.token);
            }

            let token_resp: TokenResponse = resp
                .json()
                .await
                .context("Failed to parse token response")?;

            return Ok(token_resp.token);
        }

        // No auth required
        Ok(String::new())
    }

    /// Get auth challenge from WWW-Authenticate header.
    async fn get_auth_challenge(&self, registry: &str) -> Result<HashMap<String, String>> {
        let api_url = Self::get_api_url(registry);
        let url = format!("{}/", api_url);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| map_transport_error(e, &format!("auth challenge for {registry}")))?;

        // Get WWW-Authenticate header
        if let Some(www_auth) = resp.headers().get("www-authenticate") {
            let www_auth = www_auth.to_str().unwrap_or("");
            return Ok(parse_www_authenticate(www_auth));
        }

        Ok(HashMap::new())
    }

    /// Pull an image from a registry.
    pub async fn pull(&mut self, reference: &ImageReference) -> Result<PulledImage> {
        info!("Pulling image {}", reference);

        let registry = &reference.registry;
        let repo = &reference.repository;
        let api_url = Self::get_api_url(registry);

        // Get auth token
        let token = self.get_auth_token(registry, repo).await?;

        // Get manifest
        let reference_str = reference
            .digest
            .as_ref()
            .map(|d| d.clone())
            .unwrap_or_else(|| reference.tag.clone());

        let manifest_bytes = self
            .get_manifest(&api_url, repo, &reference_str, &token)
            .await?;

        // Try to parse as an index (manifest list) first
        let manifest: oci_spec::image::ImageManifest = if let Ok(index) =
            serde_json::from_slice::<oci_spec::image::ImageIndex>(&manifest_bytes)
        {
            // This is a manifest list - select the appropriate platform
            info!(
                "Received manifest list with {} manifests",
                index.manifests().len()
            );

            // Find the manifest for our platform (linux/amd64)
            let target_manifest = index
                .manifests()
                .iter()
                .find(|m| {
                    if let Some(platform) = m.platform() {
                        // Check for linux/amd64
                        matches!(platform.os(), oci_spec::image::Os::Linux)
                            && matches!(platform.architecture(), oci_spec::image::Arch::Amd64)
                    } else {
                        false
                    }
                })
                .or_else(|| {
                    // Fallback: try to find any linux manifest
                    index.manifests().iter().find(|m| {
                        if let Some(platform) = m.platform() {
                            matches!(platform.os(), oci_spec::image::Os::Linux)
                        } else {
                            false
                        }
                    })
                })
                .ok_or_else(|| {
                    ExoError::ImageNotFound(format!(
                        "{reference} (no manifest for linux/amd64)"
                    ))
                })?;

            let manifest_digest = target_manifest.digest();
            info!("Selected manifest: {}", manifest_digest);

            // Fetch the actual manifest
            let manifest_bytes = self
                .get_manifest(&api_url, repo, &manifest_digest.to_string(), &token)
                .await?;
            serde_json::from_slice(&manifest_bytes).context("Failed to parse image manifest")?
        } else {
            // Not a manifest list, parse as regular manifest
            serde_json::from_slice(&manifest_bytes).context("Failed to parse manifest")?
        };

        info!("Image has {} layers", manifest.layers().len());

        // Save manifest
        self.store.save_oci_manifest(reference, &manifest)?;

        // Download config blob
        let config_digest = manifest.config().digest().to_string();
        let config_path = self.store.blob_path(&config_digest);
        if !config_path.exists() {
            info!("Downloading config {}", config_digest);
            self.download_blob(&api_url, repo, &config_digest, &config_path, &token)
                .await?;
        }

        // Download layers
        let mut layer_digests = Vec::new();
        for layer in manifest.layers() {
            let digest = layer.digest().to_string();
            let blob_path = self.store.blob_path(&digest);

            if blob_path.exists() {
                debug!("Layer {} already exists", digest);
            } else {
                info!("Downloading layer {}", digest);
                self.download_blob(&api_url, repo, &digest, &blob_path, &token)
                    .await?;
            }

            layer_digests.push(digest);
        }

        // Extract layers to rootfs
        self.extract_layers(reference, &manifest)?;

        info!("Successfully pulled {}", reference);

        Ok(PulledImage {
            reference: reference.clone(),
            manifest: manifest,
            config_digest,
            layer_digests,
        })
    }

    /// Get a manifest from the registry.
    async fn get_manifest(
        &self,
        api_url: &str,
        repo: &str,
        reference: &str,
        token: &str,
    ) -> Result<Vec<u8>> {
        let url = format!("{}/{}/manifests/{}", api_url, repo, reference);

        let mut request = self.client.get(&url);

        if !token.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        // Accept both OCI and Docker manifest types
        let resp = request
            .header("Accept", "application/vnd.oci.image.manifest.v1+json")
            .header(
                "Accept",
                "application/vnd.docker.distribution.manifest.v2+json",
            )
            .header(
                "Accept",
                "application/vnd.docker.distribution.manifest.list.v2+json",
            )
            .send()
            .await
            .map_err(|e| map_transport_error(e, &format!("manifest for {repo}:{reference}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_registry_status(
                status.as_u16(),
                &format!("{repo}:{reference}"),
                &body,
            )
            .into());
        }

        let bytes = resp.bytes().await.context("Failed to read manifest")?;
        Ok(bytes.to_vec())
    }

    /// Download a blob from the registry.
    async fn download_blob(
        &self,
        api_url: &str,
        repo: &str,
        digest: &str,
        dest: &Path,
        token: &str,
    ) -> Result<()> {
        let url = format!("{}/{}/blobs/{}", api_url, repo, digest);

        let mut request = self.client.get(&url);

        if !token.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let resp = request
            .send()
            .await
            .map_err(|e| map_transport_error(e, &format!("blob {digest}")))?;

        if !resp.status().is_success() {
            return Err(classify_registry_status(resp.status().as_u16(), digest, "").into());
        }

        // Stream to file
        std::fs::create_dir_all(dest.parent().unwrap())?;

        let mut file = std::fs::File::create(dest)
            .with_context(|| format!("Failed to create blob file: {:?}", dest))?;

        let mut hasher = Sha256::new();

        // Use streaming download
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Failed to read blob chunk")?;
            use std::io::Write;
            file.write_all(&chunk)?;
            hasher.update(&chunk);
        }

        // Verify digest
        let computed = format!("sha256:{:x}", hasher.finalize());

        if computed != digest {
            std::fs::remove_file(dest).ok();
            return Err(ExoError::Internal(format!(
                "digest mismatch: expected {digest}, got {computed}"
            ))
            .into());
        }

        debug!("Saved blob to {:?}", dest);
        Ok(())
    }

    /// Extract layers to rootfs.
    fn extract_layers(
        &self,
        reference: &ImageReference,
        manifest: &oci_spec::image::ImageManifest,
    ) -> Result<()> {
        let rootfs = self.store.rootfs_path(reference);

        // Clean existing rootfs
        if rootfs.exists() {
            std::fs::remove_dir_all(&rootfs)?;
        }
        std::fs::create_dir_all(&rootfs)?;

        // Extract each layer in order
        for layer in manifest.layers() {
            let digest = layer.digest().to_string();
            let blob_path = self.store.blob_path(&digest);
            crate::extract_layer(&blob_path, &rootfs)?;
        }

        Ok(())
    }
}

/// Result of pulling an image.
#[derive(Debug)]
pub struct PulledImage {
    /// Image reference.
    pub reference: ImageReference,
    /// Manifest.
    pub manifest: oci_spec::image::ImageManifest,
    /// Config digest.
    pub config_digest: String,
    /// Layer digests.
    pub layer_digests: Vec<String>,
}

/// Parse WWW-Authenticate header.
fn parse_www_authenticate(header: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();

    // Bearer realm="https://auth.docker.io/token",service="registry.docker.io"
    let header = header.trim();

    if let Some(stripped) = header.strip_prefix("Bearer ") {
        for part in stripped.split(',') {
            if let Some((key, value)) = part.split_once('=') {
                let value = value.trim_matches('"');
                result.insert(key.trim().to_string(), value.to_string());
            }
        }
    }

    result
}

// URL encoding for auth URLs
mod urlencoding {
    pub fn encode(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::classify_registry_status;
    use exo_runtime::ExoError;

    #[test]
    fn registry_statuses_map_to_taxonomy() {
        let err = classify_registry_status(404, "library/alpine:latest", "not found");
        assert!(matches!(err, ExoError::ImageNotFound(_)));
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("library/alpine:latest"));

        for status in [401, 403] {
            let err = classify_registry_status(status, "private/repo:latest", "");
            assert!(matches!(err, ExoError::RegistryAuth(_)));
            assert_eq!(err.exit_code(), 4);
            assert!(!err.retryable());
        }

        for status in [500, 502, 503] {
            let err = classify_registry_status(status, "library/alpine:latest", "");
            assert!(matches!(err, ExoError::RegistryUnavailable(_)));
            assert_eq!(err.exit_code(), 4);
            assert!(err.retryable());
        }

        let err = classify_registry_status(418, "x", "teapot");
        assert!(matches!(err, ExoError::Internal(_)));
        assert_eq!(err.exit_code(), 6);
        assert!(err.to_string().contains("418"));
    }

    #[test]
    fn typed_registry_errors_survive_anyhow() {
        let err: anyhow::Error = classify_registry_status(404, "img:tag", "").into();
        assert_eq!(exo_runtime::exit_code_for(&err), 2);
        let json = serde_json::to_value(exo_runtime::envelope_for(&err)).unwrap();
        assert_eq!(json["error"]["code"], "IMAGE_NOT_FOUND");
    }
}
