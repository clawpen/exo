//! Local secret storage for Exo.
//!
//! Secrets are values (API keys, tokens, credentials) that agents/tools need at
//! runtime but that must **not** be inherited from the ambient shell and must
//! **not** be persisted into container metadata. This store keeps secret values
//! in a restricted-permission directory, separate from container state, so that
//! `--secret NAME` can inject a value at spawn time while metadata records only
//! the secret *name*.
//!
//! Resolution order for the store directory:
//!
//! 1. `EXO_SECRETS_DIR`, when set.
//! 2. `$XDG_DATA_HOME/exo/secrets`, when set.
//! 3. `$HOME/.local/share/exo/secrets`.
//! 4. A process/user temp directory as a last resort.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Secret name validation: keep names shell/env-safe.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("secret name must not be empty");
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if !ok {
        anyhow::bail!(
            "invalid secret name '{}'; use letters, digits, '_', '-', '.'",
            name
        );
    }
    Ok(())
}

/// A filesystem-backed secret store.
pub struct SecretStore {
    dir: PathBuf,
}

impl SecretStore {
    /// Open the default secret store, creating the directory if needed.
    pub fn new() -> Result<Self> {
        Self::with_dir(default_secrets_dir())
    }

    /// Open a secret store at an explicit directory (used by tests).
    pub fn with_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create secrets directory: {:?}", dir))?;
        restrict_dir_permissions(&dir);
        Ok(Self { dir })
    }

    /// The directory backing this store.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn secret_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Store (or overwrite) a secret value.
    pub fn set(&self, name: &str, value: &str) -> Result<()> {
        validate_name(name)?;
        let path = self.secret_path(name);
        std::fs::write(&path, value.as_bytes())
            .with_context(|| format!("failed to write secret {}", name))?;
        restrict_file_permissions(&path);
        Ok(())
    }

    /// Fetch a secret value, if present.
    pub fn get(&self, name: &str) -> Result<Option<String>> {
        validate_name(name)?;
        let path = self.secret_path(name);
        match std::fs::read_to_string(&path) {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("failed to read secret {}", name)),
        }
    }

    /// List all stored secret names, sorted.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut names = vec![];
        if !self.dir.exists() {
            return Ok(names);
        }
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                names.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Remove a secret. Returns true if a secret was removed.
    pub fn remove(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let path = self.secret_path(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("failed to remove secret {}", name)),
        }
    }
}

/// Resolve the default secrets directory.
pub fn default_secrets_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("EXO_SECRETS_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("exo").join("secrets");
    }
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("exo")
            .join("secrets");
    }
    std::env::temp_dir().join("exo").join("secrets")
}

fn restrict_dir_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o700);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn restrict_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_list_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::with_dir(dir.path()).unwrap();

        assert!(store.get("TOKEN").unwrap().is_none());
        store.set("TOKEN", "s3cr3t").unwrap();
        assert_eq!(store.get("TOKEN").unwrap().as_deref(), Some("s3cr3t"));

        store.set("OTHER", "value").unwrap();
        assert_eq!(
            store.list().unwrap(),
            vec!["OTHER".to_string(), "TOKEN".to_string()]
        );

        assert!(store.remove("TOKEN").unwrap());
        assert!(!store.remove("TOKEN").unwrap());
        assert!(store.get("TOKEN").unwrap().is_none());
    }

    #[test]
    fn rejects_invalid_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::with_dir(dir.path()).unwrap();
        assert!(store.set("bad name", "x").is_err());
        assert!(store.set("../escape", "x").is_err());
        assert!(store.set("", "x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stores_with_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::with_dir(dir.path()).unwrap();
        store.set("TOKEN", "x").unwrap();
        let mode = std::fs::metadata(store.dir().join("TOKEN"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
