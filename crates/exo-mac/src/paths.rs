//! Native macOS path handling.

use crate::MacConfig;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Path translator for macOS host paths.
pub struct PathTranslator {
    _config: MacConfig,
}

impl PathTranslator {
    pub fn new(config: MacConfig) -> Self {
        Self { _config: config }
    }

    /// Normalize an existing host path for macOS execution.
    pub fn normalize_host_path(&self, path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    /// Convert a CLI mount source into a host path.
    pub fn mount_source_to_host(&self, source: &str) -> Result<PathBuf> {
        let path = Path::new(source);
        if path.is_absolute() {
            return Ok(self.normalize_host_path(path));
        }
        Ok(self.normalize_host_path(&std::env::current_dir()?.join(path)))
    }
}

impl Default for PathTranslator {
    fn default() -> Self {
        Self::new(MacConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_pass_through() {
        let translator = PathTranslator::default();
        assert!(translator
            .mount_source_to_host("/Users")
            .unwrap()
            .is_absolute());
    }
}
