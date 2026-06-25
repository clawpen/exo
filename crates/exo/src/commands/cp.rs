//! `exo cp` — copy files between host and container rootfs.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(not(windows))]
use exo_runtime::ContainerManager;

pub struct CpArgs {
    pub source: String,
    pub dest: String,
}

pub async fn execute(args: CpArgs) -> Result<()> {
    #[cfg(windows)]
    {
        return execute_windows(args).await;
    }

    #[cfg(not(windows))]
    {
        return execute_linux(args).await;
    }
}

#[cfg(windows)]
async fn execute_windows(_args: CpArgs) -> Result<()> {
    anyhow::bail!("`exo cp` is not yet supported on Windows; run inside WSL2.")
}

#[cfg(not(windows))]
async fn execute_linux(args: CpArgs) -> Result<()> {
    let (src_container, src_path) = parse_copy_spec(&args.source);
    let (dst_container, dst_path) = parse_copy_spec(&args.dest);

    let manager = ContainerManager::new()?;

    match (src_container, dst_container) {
        // container:src  host:dst   — copy from container to host
        (Some(src_name), None) => {
            let rootfs = resolve_rootfs(&manager, &src_name)?;
            let src = rootfs.join(strip_leading_slash(&src_path));
            copy_recursive(&src, &dst_path, /* to_container */ false, &rootfs)?;
        }
        // host:src  container:dst  — copy from host to container
        (None, Some(dst_name)) => {
            let rootfs = resolve_rootfs(&manager, &dst_name)?;
            let dst = rootfs.join(strip_leading_slash(&dst_path));
            copy_recursive(&src_path, &dst, /* to_container */ true, &rootfs)?;
        }
        // container:src container:dst — container-to-container copy
        (Some(src_name), Some(dst_name)) => {
            let src_rootfs = resolve_rootfs(&manager, &src_name)?;
            let dst_rootfs = resolve_rootfs(&manager, &dst_name)?;
            let src = src_rootfs.join(strip_leading_slash(&src_path));
            let dst = dst_rootfs.join(strip_leading_slash(&dst_path));
            copy_recursive(&src, &dst, /* to_container */ true, &dst_rootfs)?;
        }
        // host:src host:dst — not our job; use cp
        (None, None) => {
            anyhow::bail!(
                "At least one path must use the `container:path` syntax. \
                 Use `cp` for host-to-host copies."
            );
        }
    }

    Ok(())
}

fn parse_copy_spec(spec: &str) -> (Option<String>, PathBuf) {
    if let Some((container, path)) = spec.split_once(':') {
        (Some(container.to_string()), PathBuf::from(path))
    } else {
        (None, PathBuf::from(spec))
    }
}

#[cfg(not(windows))]
fn strip_leading_slash(path: &Path) -> PathBuf {
    path.strip_prefix("/").unwrap_or(path).to_path_buf()
}

/// Resolve the rootfs path for a container, whether running or stopped.
#[cfg(not(windows))]
fn resolve_rootfs(manager: &ContainerManager, name: &str) -> Result<PathBuf> {
    use exo_runtime::{ContainerHandle, ContainerManager};

    let metadata = manager
        .find(name)?
        .ok_or_else(|| anyhow::anyhow!("Container not found: {}", name))?;
    let handle = ContainerHandle::new(metadata.name, metadata.config);
    let path = handle.rootfs_path();
    if !path.exists() {
        anyhow::bail!(
            "Container rootfs does not exist at {}. The container may have been removed.",
            path.display()
        );
    }
    Ok(path)
}

/// Copy a file or directory recursively.
#[cfg(not(windows))]
fn copy_recursive(src: &Path, dst: impl AsRef<Path>, _to_container: bool, _rootfs: &Path) -> Result<()> {
    let dst = dst.as_ref();

    if !src.exists() {
        anyhow::bail!("Source path does not exist: {}", src.display());
    }

    // If destination is a directory, place the source inside it.
    let dst = if dst.exists() && dst.is_dir() {
        dst.join(src.file_name().ok_or_else(|| anyhow::anyhow!("Source has no file name"))?)
    } else {
        dst.to_path_buf()
    };

    if src.is_file() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all::<&Path>(parent)
                .with_context(|| format!("Failed to create destination parent: {}", parent.display()))?;
        }
        fs::copy(src, &dst)
            .with_context(|| format!("Failed to copy {} to {}", src.display(), dst.display()))?;
        tracing::info!("Copied {} to {}", src.display(), dst.display());
        return Ok(());
    }

    if src.is_dir() {
        fs::create_dir_all(&dst)
            .with_context(|| format!("Failed to create destination directory: {}", dst.display()))?;

        for entry in fs::read_dir(src)
            .with_context(|| format!("Failed to read source directory: {}", src.display()))?
        {
            let entry = entry?;
            let src_child = entry.path();
            let dst_child = dst.join(entry.file_name());
            copy_recursive(&src_child, &dst_child, _to_container, _rootfs)?;
        }
        tracing::info!("Copied directory {} to {}", src.display(), dst.display());
        return Ok(());
    }

    anyhow::bail!("Unsupported source path type: {}", src.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_copy_spec() {
        let (c, p) = parse_copy_spec("mycontainer:/etc/hostname");
        assert_eq!(c, Some("mycontainer".to_string()));
        assert_eq!(p, PathBuf::from("/etc/hostname"));

        let (c, p) = parse_copy_spec("/tmp/file.txt");
        assert_eq!(c, None);
        assert_eq!(p, PathBuf::from("/tmp/file.txt"));
    }

    #[test]
    #[cfg(not(windows))]
    fn test_copy_recursive_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src.txt");
        let dst_dir = tmp.path().join("dst");
        fs::create_dir_all(&dst_dir).unwrap();
        fs::write(&src, "hello").unwrap();

        copy_recursive(&src, &dst_dir, false, tmp.path()).unwrap();

        let dst = dst_dir.join("src.txt");
        assert!(dst.exists());
        assert_eq!(fs::read_to_string(dst).unwrap(), "hello");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_copy_recursive_directory() {
        let tmp = TempDir::new().unwrap();
        let src_dir = tmp.path().join("src");
        let dst_dir = tmp.path().join("dst");
        fs::create_dir_all(src_dir.join("nested")).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        fs::write(src_dir.join("a.txt"), "a").unwrap();
        fs::write(src_dir.join("nested").join("b.txt"), "b").unwrap();

        copy_recursive(&src_dir, &dst_dir, false, tmp.path()).unwrap();

        assert!(dst_dir.join("src").join("a.txt").exists());
        assert!(dst_dir.join("src").join("nested").join("b.txt").exists());
    }
}
