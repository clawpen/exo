//! OpenClaw Image Handling
//!
//! OCI image format support and storage.

use anyhow::Result;

/// OCI image reference (e.g., "python:3.12" or "docker.io/library/python:3.12")
#[derive(Debug, Clone)]
pub struct ImageReference {
    /// Registry hostname (e.g., "docker.io")
    pub registry: String,
    /// Repository name (e.g., "library/python")
    pub repository: String,
    /// Tag (e.g., "3.12")
    pub tag: String,
    /// Digest (alternative to tag)
    pub digest: Option<String>,
}

impl ImageReference {
    /// Parse an image reference from a string.
    pub fn parse(s: &str) -> Result<Self> {
        let (registry, repo_part) = if s.contains('/') {
            let parts: Vec<&str> = s.splitn(2, '/').collect();
            (parts[0].to_string(), parts[1].to_string())
        } else {
            ("docker.io".to_string(), s.to_string())
        };

        let (repository, tag) = if repo_part.contains(':') {
            let parts: Vec<&str> = repo_part.splitn(2, ':').collect();
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (repo_part, "latest".to_string())
        };

        // Normalize library prefix
        let repository = if !repository.contains('/') && registry == "docker.io" {
            format!("library/{}", repository)
        } else {
            repository
        };

        Ok(Self {
            registry,
            repository,
            tag,
            digest: None,
        })
    }

    /// Get the full reference string.
    pub fn full_name(&self) -> String {
        format!("{}/{}:{}", self.registry, self.repository, self.tag)
    }
}

impl std::fmt::Display for ImageReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}:{}", self.registry, self.repository, self.tag)
    }
}

impl std::str::FromStr for ImageReference {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

/// Image store - manages local image storage.
pub struct ImageStore {
    root_path: std::path::PathBuf,
}

impl ImageStore {
    /// Create a new image store.
    pub fn new(root_path: std::path::PathBuf) -> Self {
        std::fs::create_dir_all(root_path.join("blobs")).ok();
        std::fs::create_dir_all(root_path.join("manifests")).ok();
        Self { root_path }
    }

    /// Get the default image store path.
    pub fn default_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/var/lib/openclaw/images")
    }

    /// Check if an image exists locally.
    pub fn has_image(&self, reference: &ImageReference) -> bool {
        let manifest_path = self.root_path
            .join("manifests")
            .join(format!("{}:{}.json", reference.repository, reference.tag));
        manifest_path.exists()
    }

    /// List all locally stored images.
    pub fn list_images(&self) -> Result<Vec<ImageReference>> {
        let mut images = vec![];
        let manifests_dir = self.root_path.join("manifests");

        if let Ok(entries) = std::fs::read_dir(manifests_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(repo) = name_str.strip_suffix(".json") {
                    if let Ok(reference) = ImageReference::parse(&format!("docker.io/{}", repo.replace(':', ":"))) {
                        images.push(reference);
                    }
                }
            }
        }

        Ok(images)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_reference_parse() {
        let ref1 = ImageReference::parse("python:3.12").unwrap();
        assert_eq!(ref1.registry, "docker.io");
        assert_eq!(ref1.repository, "library/python");
        assert_eq!(ref1.tag, "3.12");

        let ref2 = ImageReference::parse("ghcr.io/myorg/myimage:v1.0").unwrap();
        assert_eq!(ref2.registry, "ghcr.io");
        assert_eq!(ref2.repository, "myorg/myimage");
        assert_eq!(ref2.tag, "v1.0");
    }

    #[test]
    fn test_image_reference_display() {
        let reference = ImageReference::parse("python:3.12").unwrap();
        assert_eq!(reference.to_string(), "docker.io/library/python:3.12");
    }
}
