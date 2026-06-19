//! Image reference parsing.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// OCI image reference (e.g., "python:3.12" or "docker.io/library/python:3.12")
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        // Handle digest format (image@sha256:...)
        let (repo_part, digest) = if s.contains('@') {
            let parts: Vec<&str> = s.splitn(2, '@').collect();
            (parts[0].to_string(), Some(parts[1].to_string()))
        } else {
            (s.to_string(), None)
        };

        let (registry, repo_with_tag) = if repo_part.contains('/') {
            let parts: Vec<&str> = repo_part.splitn(2, '/').collect();
            // Check if first part looks like a registry (has . or : or is localhost)
            let first = parts[0];
            if first.contains('.') || first.contains(':') || first == "localhost" {
                (first.to_string(), parts[1].to_string())
            } else {
                // It's a namespace, not a registry
                ("docker.io".to_string(), repo_part)
            }
        } else {
            ("docker.io".to_string(), repo_part)
        };

        let (repository, tag) = if let Some(ref _d) = digest {
            // If we have a digest, tag is not required
            (repo_with_tag, "latest".to_string())
        } else if repo_with_tag.contains(':') {
            let parts: Vec<&str> = repo_with_tag.rsplitn(2, ':').collect();
            if parts.len() == 2 {
                (parts[1].to_string(), parts[0].to_string())
            } else {
                (parts[0].to_string(), "latest".to_string())
            }
        } else {
            (repo_with_tag, "latest".to_string())
        };

        // Normalize library prefix for Docker Hub
        let repository = if !repository.contains('/') && registry == "docker.io" {
            format!("library/{}", repository)
        } else {
            repository
        };

        Ok(Self {
            registry,
            repository,
            tag,
            digest,
        })
    }

    /// Get the full reference string.
    pub fn full_name(&self) -> String {
        if let Some(ref digest) = self.digest {
            format!("{}/{}@{}", self.registry, self.repository, digest)
        } else {
            format!("{}/{}:{}", self.registry, self.repository, self.tag)
        }
    }
    
    /// Get a filesystem-safe name for this image.
    pub fn fs_name(&self) -> String {
        format!("{}_{}", 
            self.repository.replace('/', "_"),
            self.tag)
    }
}

impl std::fmt::Display for ImageReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.full_name())
    }
}

impl std::str::FromStr for ImageReference {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_never_panics_on_adversarial_input() {
        // Reference parsing runs on untrusted CLI/registry input; it may return
        // an error but must never panic (indexing, slicing, unwrap).
        let inputs = [
            "", ":", "@", "/", "::", "@@", "//", ":/@", "a:", ":b", "a@", "@b",
            "a:b:c", "a/b/c/d", "a@sha256:", "@sha256:abc", "registry.io/",
            "/leading", "trailing/", "host:port/", ":tag", "a:b@c:d",
            "🦀:latest", "a b c", "\t\n", "localhost:5000/",
        ];
        for s in inputs {
            let _ = ImageReference::parse(s); // must not panic
        }
        let _ = ImageReference::parse(&"x".repeat(5000));
    }

    #[test]
    fn test_parse_simple() {
        let r = ImageReference::parse("alpine").unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "library/alpine");
        assert_eq!(r.tag, "latest");
    }

    #[test]
    fn test_parse_with_tag() {
        let r = ImageReference::parse("alpine:3.19").unwrap();
        assert_eq!(r.registry, "docker.io");
        assert_eq!(r.repository, "library/alpine");
        assert_eq!(r.tag, "3.19");
    }

    #[test]
    fn test_parse_with_registry() {
        let r = ImageReference::parse("ghcr.io/myorg/myimage:v1").unwrap();
        assert_eq!(r.registry, "ghcr.io");
        assert_eq!(r.repository, "myorg/myimage");
        assert_eq!(r.tag, "v1");
    }

    #[test]
    fn test_parse_with_digest() {
        let r = ImageReference::parse("alpine@sha256:abc123").unwrap();
        assert_eq!(r.digest, Some("sha256:abc123".to_string()));
    }
}
