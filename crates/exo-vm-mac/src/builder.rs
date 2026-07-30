use crate::config::VmConfig;
use crate::image::download_file_if_missing;
use crate::paths;
use std::fs::OpenOptions;
use std::io::{Seek, Write};
use std::path::PathBuf;
use tracing::info;

pub struct ImagePaths {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub disk: PathBuf,
}

/// Ensure the VM image artifacts exist. If `force` is true, re-download/rebuild.
pub async fn ensure_image(config: &VmConfig, force: bool) -> anyhow::Result<ImagePaths> {
    let kernel = paths::kernel_path()?;
    let base_initrd = paths::base_initrd_path()?;
    let exo_initrd = paths::exo_initrd_path()?;
    let disk = paths::disk_path()?;

    if force {
        let _ = std::fs::remove_file(&kernel);
        let _ = std::fs::remove_file(&base_initrd);
        let _ = std::fs::remove_file(&exo_initrd);
        let _ = std::fs::remove_file(&disk);
    }

    download_file_if_missing(&config.kernel_url, &kernel).await?;
    download_file_if_missing(&config.initrd_url, &base_initrd).await?;

    if force || !exo_initrd.exists() {
        build_guest_initrd(&base_initrd, &exo_initrd)?;
    }

    if force || !disk.exists() {
        create_raw_disk(&disk, 100 * 1024 * 1024)?;
    }

    Ok(ImagePaths {
        kernel,
        initrd: exo_initrd,
        disk,
    })
}

fn create_raw_disk(path: &std::path::Path, size: u64) -> anyhow::Result<()> {
    info!("Creating raw disk {} ({} bytes)", path.display(), size);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    let zero_block = vec![0u8; 64 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let chunk = std::cmp::min(remaining, zero_block.len() as u64) as usize;
        file.write_all(&zero_block[..chunk])?;
        remaining -= chunk as u64;
    }
    Ok(())
}

fn create_sparse_disk(path: &std::path::Path, size: u64) -> anyhow::Result<()> {
    info!("Creating sparse disk {} ({} bytes)", path.display(), size);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    if size > 0 {
        file.seek(std::io::SeekFrom::Start(size - 1))?;
        file.write_all(&[0])?;
    }
    Ok(())
}

fn build_guest_initrd(base: &std::path::Path, out: &std::path::Path) -> anyhow::Result<()> {
    info!("Building Exo initramfs at {}", out.display());
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path();

    // Extract the base Alpine initramfs (gzip-compressed cpio).
    let extract = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cd {} && gunzip -c {} | cpio -id --quiet",
            shell_escape(temp_path),
            shell_escape(base)
        ))
        .output()?;
    if !extract.status.success() {
        let stderr = String::from_utf8_lossy(&extract.stderr);
        anyhow::bail!("failed to extract base initramfs: {}", stderr);
    }

    // Embed a guest agent if one has been built; otherwise use a shell fallback
    // that proves the VM boots and stays alive.
    let init_script: Vec<u8>;
    let agent_path = paths::guest_agent_binary_path()?;
    if agent_path.exists() {
        let bin_dir = temp_path.join("usr").join("local").join("bin");
        std::fs::create_dir_all(&bin_dir)?;

        let agent_dest = bin_dir.join("exo-vm-guest-init");
        std::fs::copy(&agent_path, &agent_dest)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&agent_dest)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&agent_dest, perms)?;
        }

        // Also embed exo-agent if it has been built for the guest; exoclaw runs
        // invoke it inside the VM.
        let exo_agent_path = paths::exo_agent_binary_path()?;
        if exo_agent_path.exists() {
            let exo_agent_dest = bin_dir.join("exo-agent");
            std::fs::copy(&exo_agent_path, &exo_agent_dest)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&exo_agent_dest)?.permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&exo_agent_dest, perms)?;
            }
            info!("Embedded exo-agent in initramfs");
        }

        init_script = b"#!/bin/sh\nmkdir -p /proc /sys /dev /tmp\nmount -t proc proc /proc\nmount -t sysfs sysfs /sys\nmount -t devtmpfs devtmpfs /dev 2>/dev/null || true\nexec /usr/local/bin/exo-vm-guest-init\n".to_vec();
    } else {
        init_script = br#"#!/bin/sh
# Exo microVM fallback init - prove the VM boots and keep it alive.
exec 1>/dev/console 2>/dev/console
echo "exo-vm-guest: shell fallback init started"
mkdir -p /proc /sys /dev /tmp
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
while true; do
    echo "exo-vm-guest: heartbeat"
    sleep 10
done
"#
        .to_vec();
    }

    let init_path = temp_path.join("init");
    std::fs::write(&init_path, init_script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&init_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&init_path, perms)?;
    }

    // Repack as gzip-compressed newc cpio.
    let repack = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cd {} && find . -print0 | cpio --null -o --format=newc --quiet | gzip -c > {}",
            shell_escape(temp_path),
            shell_escape(out)
        ))
        .output()?;
    if !repack.status.success() {
        let stderr = String::from_utf8_lossy(&repack.stderr);
        anyhow::bail!("failed to repack initramfs: {}", stderr);
    }

    let size = std::fs::metadata(out)?.len();
    info!("Wrote Exo initramfs to {} ({} bytes)", out.display(), size);
    Ok(())
}

fn shell_escape(path: &std::path::Path) -> String {
    // Sufficient for paths that don't contain single quotes.
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}
