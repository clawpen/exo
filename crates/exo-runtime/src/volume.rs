//! Named volume storage for Exo.
//!
//! Volumes are named, persistent directories that can be mounted into
//! containers with `-v name:/path`. Unlike ad-hoc bind mounts (which map an
//! explicit host path), named volumes are managed by Exo in a dedicated store
//! so they survive container removal and can be shared between containers and
//! backends (native macOS today, the Linux microVM guest once wired).
//!
//! Store directory resolution:
//!
//! 1. `EXO_VOLUMES_DIR`, when set.
//! 2. `$XDG_DATA_HOME/exo/volumes`, when set.
//! 3. `$HOME/.local/share/exo/volumes`.
//! 4. A process/user temp directory as a last resort.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Validate a volume name: keep names filesystem/CLI safe.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("volume name must not be empty");
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !ok {
        anyhow::bail!(
            "invalid volume name '{}'; use letters, digits, '_', '-', '.'",
            name
        );
    }
    if name == "." || name == ".." {
        anyhow::bail!("invalid volume name '{}'", name);
    }
    Ok(())
}

/// A string source refers to a named volume when it is a bare, valid volume
/// name (no path separator and not absolute). Anything containing '/' is
/// treated as a bind-mount host path.
pub fn is_volume_reference(source: &str) -> bool {
    !source.contains('/') && validate_name(source).is_ok()
}

/// A filesystem-backed named volume store.
pub struct VolumeStore {
    dir: PathBuf,
}

impl VolumeStore {
    pub fn new() -> Result<Self> {
        Self::with_dir(default_volumes_dir())
    }

    pub fn with_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create volumes directory: {:?}", dir))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Absolute path backing a named volume (does not require it to exist).
    pub fn path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir.join(name))
    }

    pub fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.path(name)?.is_dir())
    }

    /// Create a volume if missing; returns its backing path.
    pub fn create(&self, name: &str) -> Result<PathBuf> {
        let path = self.path(name)?;
        std::fs::create_dir_all(&path)
            .with_context(|| format!("failed to create volume {}", name))?;
        Ok(path)
    }

    /// Ensure a volume exists and return its path (alias for `create`).
    pub fn ensure(&self, name: &str) -> Result<PathBuf> {
        self.create(name)
    }

    pub fn list(&self) -> Result<Vec<String>> {
        let mut names = vec![];
        if !self.dir.exists() {
            return Ok(names);
        }
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                names.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Remove a volume and its contents. Returns true if it existed.
    pub fn remove(&self, name: &str) -> Result<bool> {
        let path = self.path(name)?;
        match std::fs::remove_dir_all(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("failed to remove volume {}", name)),
        }
    }
}

pub fn default_volumes_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("EXO_VOLUMES_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("exo").join("volumes");
    }
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("exo")
            .join("volumes");
    }
    std::env::temp_dir().join("exo").join("volumes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_list_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = VolumeStore::with_dir(dir.path()).unwrap();

        assert!(!store.exists("data").unwrap());
        let path = store.create("data").unwrap();
        assert!(path.is_dir());
        assert!(store.exists("data").unwrap());

        store.create("cache").unwrap();
        assert_eq!(
            store.list().unwrap(),
            vec!["cache".to_string(), "data".to_string()]
        );

        assert!(store.remove("data").unwrap());
        assert!(!store.remove("data").unwrap());
        assert!(!store.exists("data").unwrap());
    }

    #[test]
    fn rejects_invalid_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = VolumeStore::with_dir(dir.path()).unwrap();
        assert!(store.create("bad name").is_err());
        assert!(store.create("../escape").is_err());
        assert!(store.create("").is_err());
    }

    #[test]
    fn volume_reference_detection() {
        assert!(is_volume_reference("data"));
        assert!(is_volume_reference("my-cache.1"));
        assert!(!is_volume_reference("/host/path"));
        assert!(!is_volume_reference("./rel"));
        assert!(!is_volume_reference("a/b"));
    }
}
