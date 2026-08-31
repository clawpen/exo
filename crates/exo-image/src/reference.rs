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

        validate_reference(s, &registry, &repository, &tag, digest.as_deref())?;

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
        format!("{}_{}", self.repository.replace('/', "_"), self.tag)
    }
}

/// Validate a parsed reference against the OCI/distribution name rules so
/// malformed references fail fast with `INVALID_INPUT` (exit 5) instead of
/// after a network round-trip. Rules follow docker/distribution:
/// repository path components are `[a-z0-9]+([._-][a-z0-9]+)*`, tags are
/// `[\w][\w.-]{0,127}`, registries are host[:port], digests are `algo:hex`.
fn validate_reference(
    original: &str,
    registry: &str,
    repository: &str,
    tag: &str,
    digest: Option<&str>,
) -> Result<()> {
    let invalid = |why: String| {
        exo_runtime::ExoError::InvalidInput(format!("invalid image reference '{original}': {why}"))
    };

    if registry.is_empty()
        || !registry
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
    {
        return Err(invalid(format!("bad registry '{registry}'")).into());
    }

    if repository.is_empty() {
        return Err(invalid("empty repository".to_string()).into());
    }
    for component in repository.split('/') {
        let valid = !component.is_empty()
            && component
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'));
        if !valid {
            return Err(invalid(format!(
                "bad repository component '{component}' (must be lowercase [a-z0-9._-])"
            ))
            .into());
        }
    }

    let valid_tag = !tag.is_empty()
        && tag.len() <= 128
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid_tag {
        return Err(invalid(format!("bad tag '{tag}'")).into());
    }

    if let Some(d) = digest {
        let valid = d.split_once(':').is_some_and(|(algo, hex)| {
            !algo.is_empty()
                && algo.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && !hex.is_empty()
                && hex.chars().all(|c| c.is_ascii_hexdigit())
        });
        if !valid {
            return Err(invalid(format!("bad digest '{d}' (expected <algo>:<hex>)")).into());
        }
    }

    Ok(())
}

impl std::fmt::Display for ImageReference {    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

    #[test]
    fn test_parse_rejects_malformed_references() {
        // Uppercase repository (OCI names are lowercase)
        assert!(ImageReference::parse("INVALID").is_err());
        assert!(ImageReference::parse("myOrg/Repo:latest").is_err());
        // Empty path components
        assert!(ImageReference::parse("foo//bar").is_err());
        assert!(ImageReference::parse("/bar").is_err());
        // Bad digest shape
        assert!(ImageReference::parse("alpine@sha256:zz!!").is_err());
        // Empty tag after ':'
        assert!(ImageReference::parse("alpine:").is_err());

        // The failures are typed INVALID_INPUT (exit 5), not stringly.
        let err = ImageReference::parse("INVALID").unwrap_err();
        assert_eq!(exo_runtime::exit_code_for(&err), 5);

        // Valid references still parse.
        assert!(ImageReference::parse("localhost:5000/myimg:v1.2").is_ok());
        assert!(ImageReference::parse("ghcr.io/org/img@sha256:abc123").is_ok());
    }
}
