//! Path translation between Windows and WSL2.
//!
//! This module handles conversion between Windows paths and WSL2 paths,
//! which is essential for volume mounts and file operations across the boundary.

use crate::WslConfig;

/// Path translator for Windows <-> WSL2 conversions.
pub struct PathTranslator {
    config: WslConfig,
}

impl PathTranslator {
    /// Create a new path translator.
    pub fn new(config: WslConfig) -> Self {
        Self { config }
    }

    /// Convert a Windows path to WSL2 path format.
    ///
    /// # Examples
    /// ```ignore
    /// C:\Users\foo\bar -> /mnt/c/Users/foo/bar
    /// F:\Software\exo -> /mnt/f/Software/exo
    /// \\wsl$\Ubuntu\home\user -> /home/user
    /// ```
    pub fn windows_to_wsl(&self, windows_path: &str) -> String {
        // Handle UNC WSL paths: \\wsl$\distro\path -> /path
        if windows_path.starts_with(r"\\wsl$\") || windows_path.starts_with(r"\\wsl.localhost\") {
            return self.unc_to_wsl(windows_path);
        }

        // Convert backslashes to forward slashes
        let normalized = windows_path.replace('\\', "/");

        // Handle drive letters: C:\ -> /mnt/c/
        if let Some(_drive_part) = normalized.strip_prefix('/') {
            // Already looks like a Unix path, maybe already converted
            return normalized;
        }

        // Check for Windows drive pattern (C:/, D:/, etc.)
        if normalized.len() >= 2 {
            let chars: Vec<char> = normalized.chars().collect();
            // Pattern: C:/path where chars[0] is drive letter and chars[1] is colon
            if chars.len() >= 2 && chars[0].is_ascii_alphabetic() && chars[1] == ':' {
                let drive = chars[0].to_lowercase();
                let rest = if normalized.len() > 2 { &normalized[2..] } else { "" };

                if rest.is_empty() || rest == "/" {
                    return format!("/mnt/{}", drive);
                }
                if rest.starts_with('/') {
                    return format!("/mnt/{}/{}", drive, &rest[1..]);
                }
                return format!("/mnt/{}/{}", drive, rest);
            }
        }

        // If no drive letter found, return as-is (might already be a WSL path)
        normalized
    }

    /// Convert a WSL2 path to Windows path format.
    ///
    /// # Examples
    /// ```ignore
    /// /mnt/c/Users/foo/bar -> C:\Users\foo\bar
    /// /home/user/file -> \\wsl$\exo\home\user\file
    /// ```
    pub fn wsl_to_windows(&self, wsl_path: &str) -> String {
        // Handle /mnt/{drive} pattern
        if let Some(rest) = wsl_path.strip_prefix("/mnt/") {
            if let Some((drive, path)) = rest.split_once('/') {
                // Ensure drive is a single letter
                if drive.len() == 1 && drive.chars().next().map_or(false, |c| c.is_ascii_alphabetic()) {
                    let drive_upper = drive.to_uppercase();
                    let rest_with_slash = if path.is_empty() { "\\" } else { &format!("\\{}", path.replace('/', "\\")) };
                    return format!("{}:{}", drive_upper, rest_with_slash);
                }
            }
        }

        // For paths inside WSL (not under /mnt), convert to UNC path
        self.wsl_to_unc(wsl_path)
    }

    /// Convert a UNC WSL path to actual WSL path.
    ///
    /// \\wsl$\exo\home\user -> /home/user
    /// \\wsl.localhost\exo\home\user -> /home/user
    fn unc_to_wsl(&self, unc_path: &str) -> String {
        let normalized = unc_path.replace('\\', "/");

        // Strip \\wsl$\ or \\wsl.localhost\
        let path = normalized
            .strip_prefix("//wsl$/")
            .or_else(|| normalized.strip_prefix("//wsl.localhost/"))
            .unwrap_or(&normalized);

        // Check if distro name matches
        if let Some(rest) = path.strip_prefix(&format!("{}/", self.config.distro_name)) {
            return format!("/{}", rest);
        }

        // Different distro: \\wsl$\Ubuntu\home -> /home (but might need translation)
        if let Some((_, rest)) = path.split_once('/') {
            return format!("/{}", rest);
        }

        format!("/{}", path)
    }

    /// Convert a WSL path to Windows UNC path.
    ///
    /// /home/user/file -> \\wsl$\exo\home\user\file
    pub fn wsl_to_unc(&self, wsl_path: &str) -> String {
        let path = wsl_path.trim_start_matches('/');
        let distro = &self.config.distro_name;
        format!(r"\\wsl$\{}\{}", distro, path.replace('/', "\\"))
    }

    /// Convert a Windows path to UNC path.
    ///
    /// C:\Users\foo -> \\wsl$\exo\mnt\c\Users\foo
    pub fn windows_to_unc(&self, windows_path: &str) -> String {
        let wsl_path = self.windows_to_wsl(windows_path);
        self.wsl_to_unc(&wsl_path)
    }

    /// Check if a path is a Windows path.
    pub fn is_windows_path(&self, path: &str) -> bool {
        path.contains('\\') || path.chars().nth(1) == Some(':')
    }

    /// Check if a path is a WSL path.
    pub fn is_wsl_path(&self, path: &str) -> bool {
        path.starts_with("/mnt/") || path.starts_with("/home/") || path.starts_with("/root/")
    }

    /// Check if a path is a UNC WSL path.
    pub fn is_unc_path(&self, path: &str) -> bool {
        path.starts_with(r"\\wsl$\") || path.starts_with(r"\\wsl.localhost\")
    }

    /// Normalize a path for the current platform.
    ///
    /// On Windows: ensures backslashes
    /// In WSL: ensures forward slashes
    pub fn normalize(&self, path: &str) -> String {
        #[cfg(windows)]
        {
            if self.is_windows_path(path) {
                path.replace('/', "\\")
            } else {
                path.replace('\\', "/")
            }
        }
        #[cfg(not(windows))]
        {
            path.replace('\\', "/")
        }
    }

    /// Join two path components, handling Windows/WSL differences.
    pub fn join(&self, base: &str, leaf: &str) -> String {
        let separator = if self.is_windows_path(base) { "\\" } else { "/" };
        let base_trimmed = base.trim_end_matches(&['/', '\\'][..]);
        let leaf_trimmed = leaf.trim_start_matches(&['/', '\\'][..]);
        format!("{}{}{}", base_trimmed, separator, leaf_trimmed)
    }
}

impl Default for PathTranslator {
    fn default() -> Self {
        Self::new(WslConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_to_wsl_drive_c() {
        let translator = PathTranslator::default();
        assert_eq!(
            translator.windows_to_wsl(r"C:\Users\foo\bar"),
            "/mnt/c/Users/foo/bar"
        );
    }

    #[test]
    fn test_windows_to_wsl_drive_f() {
        let translator = PathTranslator::default();
        assert_eq!(
            translator.windows_to_wsl(r"F:\Software\exo"),
            "/mnt/f/Software/exo"
        );
    }

    #[test]
    fn test_wsl_to_windows() {
        let translator = PathTranslator::default();
        assert_eq!(
            translator.wsl_to_windows("/mnt/c/Users/foo/bar"),
            r"C:\Users\foo\bar"
        );
        assert_eq!(
            translator.wsl_to_windows("/mnt/d/projects/test"),
            r"D:\projects\test"
        );
    }

    #[test]
    fn test_wsl_to_unc() {
        let translator = PathTranslator::default();
        assert_eq!(
            translator.wsl_to_unc("/home/user/file.txt"),
            r"\\wsl$\Ubuntu\home\user\file.txt"
        );
    }

    #[test]
    fn test_is_windows_path() {
        let translator = PathTranslator::default();
        assert!(translator.is_windows_path(r"C:\Users\foo"));
        assert!(translator.is_windows_path(r"D:\path"));
        assert!(!translator.is_windows_path("/mnt/c/Users"));
        assert!(!translator.is_windows_path("/home/user"));
    }

    #[test]
    fn test_is_wsl_path() {
        let translator = PathTranslator::default();
        assert!(translator.is_wsl_path("/mnt/c/Users"));
        assert!(translator.is_wsl_path("/home/user"));
        assert!(!translator.is_wsl_path(r"C:\Users"));
    }

    #[test]
    fn test_join() {
        let translator = PathTranslator::default();
        assert_eq!(
            translator.join("/mnt/c/Users", "foo/bar"),
            "/mnt/c/Users/foo/bar"
        );
        assert_eq!(
            translator.join(r"C:\Users", r"foo\bar"),
            r"C:\Users\foo\bar"
        );
    }
}
