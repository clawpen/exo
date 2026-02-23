//! Filesystem mounting between Windows and WSL2.

use crate::WslConfig;
use std::path::Path;

/// Handles mounting Windows directories into WSL2 containers.
pub struct WslMount {
    config: WslConfig,
}

impl WslMount {
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// Convert a Windows path to WSL path.
    ///
    /// C:\Users\foo\bar → /mnt/c/Users/foo/bar
    pub fn windows_to_wsl(&self, windows_path: &str) -> String {
        let path = windows_path.replace('\\', "/");

        if path.len() >= 2 && path.as_bytes()[1] == b':' {
            let drive = path.chars().next().unwrap().to_lowercase();
            let rest = &path[2..];
            format!("/mnt/{}/{}", drive, rest)
        } else {
            path
        }
    }

    /// Convert a WSL path to Windows path.
    ///
    /// /mnt/c/Users/foo/bar → C:\Users\foo\bar
    pub fn wsl_to_windows(&self, wsl_path: &str) -> String {
        if let Some(rest) = wsl_path.strip_prefix("/mnt/") {
            if let Some((drive, path)) = rest.split_once('/') {
                let drive_upper = drive.to_uppercase();
                return format!("{}:\\{}", drive_upper, path.replace('/', "\\"));
            }
        }
        // Handle \\wsl$\distro\path format
        wsl_path.replace('/', "\\")
    }

    /// Get the Windows UNC path for a WSL file.
    ///
    /// /home/user/file → \\wsl$\openclaw\home\user\file
    pub fn wsl_to_unc(&self, wsl_path: &str) -> String {
        format!(
            "\\\\wsl$\\{}\\{}",
            self.config.distro_name,
            wsl_path.trim_start_matches('/').replace('/', "\\")
        )
    }

    /// Check if a Windows path is accessible from WSL.
    pub fn is_accessible(&self, windows_path: &str) -> bool {
        Path::new(windows_path).exists()
    }

    /// Create a mount specification for container use.
    pub fn mount_spec(
        &self,
        windows_source: &str,
        container_target: &str,
        readonly: bool,
    ) -> MountSpec {
        MountSpec {
            source: self.windows_to_wsl(windows_source),
            target: container_target.to_string(),
            readonly,
        }
    }

    /// Get all available Windows drives as WSL mount points.
    pub fn windows_drives(&self) -> Vec<(String, String)> {
        let mut drives = vec![];

        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            if Path::new(&drive).exists() {
                let wsl_path = format!("/mnt/{}", (letter as char).to_lowercase());
                drives.push((drive, wsl_path));
            }
        }

        drives
    }
}

#[derive(Debug, Clone)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_to_wsl() {
        let mount = WslMount::new(WslConfig::default());

        assert_eq!(
            mount.windows_to_wsl(r"C:\Users\foo\bar"),
            "/mnt/c/Users/foo/bar"
        );
        assert_eq!(
            mount.windows_to_wsl(r"D:\projects\test"),
            "/mnt/d/projects/test"
        );
    }

    #[test]
    fn test_wsl_to_windows() {
        let mount = WslMount::new(WslConfig::default());

        assert_eq!(
            mount.wsl_to_windows("/mnt/c/Users/foo/bar"),
            r"C:\Users\foo\bar"
        );
    }

    #[test]
    fn test_wsl_to_unc() {
        let mount = WslMount::new(WslConfig::default());

        assert_eq!(
            mount.wsl_to_unc("/home/user/file.txt"),
            r"\\wsl$\openclaw\home\user\file.txt"
        );
    }
}
