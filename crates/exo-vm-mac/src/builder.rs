use crate::config::VmConfig;
use crate::image::download_file_if_missing;
use crate::paths;
use std::fs::OpenOptions;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ImagePaths {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub disk: PathBuf,
}

/// Files the guest needs for persistent-state disk support, fetched from Debian
/// at init time and cached under the Exo VM state dir.
struct DiskSupportPackages {
    virtio_blk_ko: PathBuf,
    ext4_ko: PathBuf,
    jbd2_ko: PathBuf,
    mke2fs: PathBuf,
    libext2fs: PathBuf,
    libe2p: PathBuf,
    libcom_err: PathBuf,
}

/// Debian installer module Packages index for arm64. The installer module
/// udebs are published alongside the netboot kernel/initrd, so the kernel ABI
/// found in the base initramfs always has matching module packages here.
const INSTALLER_PACKAGES_URL: &str =
    "https://deb.debian.org/debian/dists/stable/main/debian-installer/binary-arm64/Packages.gz";

/// Userspace e2fsprogs packages for the guest (pinned to the Debian stable
/// versions validated against the netboot glibc initramfs).
const E2FSPROGS_DEB_URL: &str = "https://deb.debian.org/debian/pool/main/e/e2fsprogs/e2fsprogs_1.47.2-3+b11_arm64.deb";
const LIBEXT2FS_DEB_URL: &str = "https://deb.debian.org/debian/pool/main/e/e2fsprogs/libext2fs2t64_1.47.2-3+b11_arm64.deb";
const LIBCOM_ERR_DEB_URL: &str = "https://deb.debian.org/debian/pool/main/e/e2fsprogs/libcom-err2_1.47.2-3+b11_arm64.deb";

/// Size of the persistent guest-state disk (sparse, so it only consumes host
/// space as the guest writes to it).
const STATE_DISK_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Ensure the VM image artifacts exist. If `force` is true, re-download/rebuild
/// everything, including the persistent-state disk (destroys guest state).
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
        let temp_dir = tempfile::tempdir()?;
        extract_base_initrd(&base_initrd, temp_dir.path())?;
        let kver = detect_kernel_version(temp_dir.path())?;
        let tools = download_disk_support_packages(&kver).await?;
        embed_disk_support(temp_dir.path(), &kver, &tools)?;
        write_init_script(temp_dir.path())?;
        repack_initrd(temp_dir.path(), &exo_initrd)?;
    }

    if force || !disk.exists() {
        create_sparse_disk(&disk, STATE_DISK_SIZE)?;
    }

    Ok(ImagePaths {
        kernel,
        initrd: exo_initrd,
        disk,
    })
}

fn extract_base_initrd(base: &Path, temp_path: &Path) -> anyhow::Result<()> {
    // Extract the base initramfs (gzip-compressed cpio).
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
    Ok(())
}

/// Kernel ABI version (e.g. `6.12.94+deb13-arm64`) from the extracted initramfs.
fn detect_kernel_version(temp_path: &Path) -> anyhow::Result<String> {
    let modules_dir = temp_path.join("lib").join("modules");
    let entries = std::fs::read_dir(&modules_dir)
        .map_err(|e| anyhow::anyhow!("read {}: {}", modules_dir.display(), e))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("modules.") {
            return Ok(name);
        }
    }
    anyhow::bail!(
        "no kernel module directory under {}; cannot match installer module packages",
        modules_dir.display()
    )
}

fn guest_tools_dir() -> anyhow::Result<PathBuf> {
    let dir = paths::exo_vm_dir()?.join("guest-tools");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Download the kernel-module udebs matching the initramfs kernel ABI and the
/// userspace e2fsprogs packages, extracting the exact files the guest needs.
async fn download_disk_support_packages(kver: &str) -> anyhow::Result<DiskSupportPackages> {
    let cache = guest_tools_dir()?;
    let out = cache.join("extracted").join(kver);
    let marker = out.join(".complete");
    if marker.exists() {
        info!("Using cached guest disk-support files at {}", out.display());
        return Ok(disk_support_paths(&out));
    }

    // Resolve the installer module udebs for this exact kernel ABI.
    let packages_gz = cache.join("installer-Packages.gz");
    let filenames = resolve_installer_udebs(&packages_gz, kver).await?;

    let ext4_udeb = cache.join(format!("ext4-modules-{kver}-di.udeb"));
    let scsi_udeb = cache.join(format!("scsi-modules-{kver}-di.udeb"));
    download_file_if_missing(&filenames.0, &ext4_udeb).await?;
    download_file_if_missing(&filenames.1, &scsi_udeb).await?;

    let e2fsprogs_deb = cache.join("e2fsprogs_arm64.deb");
    let libext2fs_deb = cache.join("libext2fs2t64_arm64.deb");
    let libcom_err_deb = cache.join("libcom-err2_arm64.deb");
    download_file_if_missing(E2FSPROGS_DEB_URL, &e2fsprogs_deb).await?;
    download_file_if_missing(LIBEXT2FS_DEB_URL, &libext2fs_deb).await?;
    download_file_if_missing(LIBCOM_ERR_DEB_URL, &libcom_err_deb).await?;

    // Extract each package and copy the files we need into `out`.
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)?;

    let modules_base = format!("lib/modules/{kver}/kernel");
    let ext4_extract = extract_deb(&ext4_udeb)?;
    copy_package_file(
        &ext4_extract,
        &format!("{modules_base}/fs/ext4/ext4.ko.xz"),
        &out.join("ext4.ko.xz"),
    )?;
    copy_package_file(
        &ext4_extract,
        &format!("{modules_base}/fs/jbd2/jbd2.ko.xz"),
        &out.join("jbd2.ko.xz"),
    )?;

    let scsi_extract = extract_deb(&scsi_udeb)?;
    copy_package_file(
        &scsi_extract,
        &format!("{modules_base}/drivers/block/virtio_blk.ko.xz"),
        &out.join("virtio_blk.ko.xz"),
    )?;

    let e2fsprogs_extract = extract_deb(&e2fsprogs_deb)?;
    copy_package_file(&e2fsprogs_extract, "usr/sbin/mke2fs", &out.join("mke2fs"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(out.join("mke2fs"))?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(out.join("mke2fs"), perms)?;
    }

    let libext2fs_extract = extract_deb(&libext2fs_deb)?;
    copy_package_file(
        &libext2fs_extract,
        "usr/lib/aarch64-linux-gnu/libext2fs.so.2.4",
        &out.join("libext2fs.so.2.4"),
    )?;
    copy_package_file(
        &libext2fs_extract,
        "usr/lib/aarch64-linux-gnu/libe2p.so.2.3",
        &out.join("libe2p.so.2.3"),
    )?;

    let libcom_err_extract = extract_deb(&libcom_err_deb)?;
    copy_package_file(
        &libcom_err_extract,
        "usr/lib/aarch64-linux-gnu/libcom_err.so.2.1",
        &out.join("libcom_err.so.2.1"),
    )?;

    std::fs::write(&marker, b"ok\n")?;
    Ok(disk_support_paths(&out))
}

fn disk_support_paths(out: &Path) -> DiskSupportPackages {
    DiskSupportPackages {
        virtio_blk_ko: out.join("virtio_blk.ko.xz"),
        ext4_ko: out.join("ext4.ko.xz"),
        jbd2_ko: out.join("jbd2.ko.xz"),
        mke2fs: out.join("mke2fs"),
        libext2fs: out.join("libext2fs.so.2.4"),
        libe2p: out.join("libe2p.so.2.3"),
        libcom_err: out.join("libcom_err.so.2.1"),
    }
}

/// Parse the installer Packages index and return full URLs of the ext4 and
/// virtio_blk module udebs for `kver`.
async fn resolve_installer_udebs(
    packages_gz: &Path,
    kver: &str,
) -> anyhow::Result<(String, String)> {
    let parse = |bytes: &[u8]| -> anyhow::Result<(String, String)> {
        let text = String::from_utf8_lossy(bytes);
        let ext4_pkg = format!("ext4-modules-{kver}-di");
        let scsi_pkg = format!("scsi-modules-{kver}-di");
        let mut ext4_url = None;
        let mut scsi_url = None;
        let mut current_pkg = String::new();
        for line in text.lines() {
            if let Some(pkg) = line.strip_prefix("Package: ") {
                current_pkg = pkg.trim().to_string();
            } else if let Some(file) = line.strip_prefix("Filename: ") {
                let file = file.trim();
                if current_pkg == ext4_pkg {
                    ext4_url = Some(format!("https://deb.debian.org/debian/{file}"));
                } else if current_pkg == scsi_pkg {
                    scsi_url = Some(format!("https://deb.debian.org/debian/{file}"));
                }
            }
        }
        match (ext4_url, scsi_url) {
            (Some(e), Some(s)) => Ok((e, s)),
            _ => anyhow::bail!(
                "installer Packages index has no module udebs for kernel {kver}; \
                 the Debian stable netboot kernel may have moved past the pinned initramfs"
            ),
        }
    };

    if packages_gz.exists() {
        let gz = std::fs::read(packages_gz)?;
        if let Ok(bytes) = gunzip(&gz) {
            if let Ok(urls) = parse(&bytes) {
                return Ok(urls);
            }
        }
        info!("Cached installer Packages index is stale; re-downloading");
        let _ = std::fs::remove_file(packages_gz);
    }

    info!("Downloading installer Packages index from {INSTALLER_PACKAGES_URL}");
    let response = reqwest::get(INSTALLER_PACKAGES_URL).await?;
    response.error_for_status_ref()?;
    let gz = response.bytes().await?;
    std::fs::write(packages_gz, &gz)?;
    let bytes = gunzip(&gz)?;
    parse(&bytes)
}

fn gunzip(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

/// Extract a `.deb`/`.udeb` (ar archive containing `data.tar.*`) into a fresh
/// temp dir and return the dir holding the package payload.
fn extract_deb(pkg: &Path) -> anyhow::Result<PathBuf> {
    let temp = tempfile::tempdir()?.into_path();
    let outer = std::process::Command::new("tar")
        .arg("xf")
        .arg(pkg)
        .arg("-C")
        .arg(&temp)
        .output()?;
    if !outer.status.success() {
        anyhow::bail!(
            "failed to unpack {}: {}",
            pkg.display(),
            String::from_utf8_lossy(&outer.stderr)
        );
    }
    let data_tar = std::fs::read_dir(&temp)?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .find(|name| name.starts_with("data.tar"))
        .ok_or_else(|| anyhow::anyhow!("no data.tar in {}", pkg.display()))?;
    let inner = std::process::Command::new("tar")
        .arg("xf")
        .arg(temp.join(&data_tar))
        .arg("-C")
        .arg(&temp)
        .output()?;
    if !inner.status.success() {
        anyhow::bail!(
            "failed to extract {} from {}: {}",
            data_tar,
            pkg.display(),
            String::from_utf8_lossy(&inner.stderr)
        );
    }
    Ok(temp)
}

fn copy_package_file(extract_root: &Path, rel: &str, dest: &Path) -> anyhow::Result<()> {
    let source = extract_root.join(rel);
    if !source.exists() {
        anyhow::bail!(
            "expected file {} not found in extracted package",
            source.display()
        );
    }
    std::fs::copy(&source, dest).map_err(|e| {
        anyhow::anyhow!("copy {} to {}: {}", source.display(), dest.display(), e)
    })?;
    Ok(())
}

/// Copy the disk-support kernel modules and e2fsprogs binaries/libs into the
/// initramfs tree.
fn embed_disk_support(
    temp_path: &Path,
    kver: &str,
    tools: &DiskSupportPackages,
) -> anyhow::Result<()> {
    let modules_base = temp_path.join("lib").join("modules").join(kver).join("kernel");
    let block_dir = modules_base.join("drivers").join("block");
    let ext4_dir = modules_base.join("fs").join("ext4");
    let jbd2_dir = modules_base.join("fs").join("jbd2");
    std::fs::create_dir_all(&block_dir)?;
    std::fs::create_dir_all(&ext4_dir)?;
    std::fs::create_dir_all(&jbd2_dir)?;
    std::fs::copy(&tools.virtio_blk_ko, block_dir.join("virtio_blk.ko.xz"))?;
    std::fs::copy(&tools.ext4_ko, ext4_dir.join("ext4.ko.xz"))?;
    std::fs::copy(&tools.jbd2_ko, jbd2_dir.join("jbd2.ko.xz"))?;

    let sbin_dir = temp_path.join("usr").join("sbin");
    std::fs::create_dir_all(&sbin_dir)?;
    let mke2fs_dest = sbin_dir.join("mke2fs");
    std::fs::copy(&tools.mke2fs, &mke2fs_dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&mke2fs_dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mke2fs_dest, perms)?;
    }

    let lib_dir = temp_path.join("usr").join("lib").join("aarch64-linux-gnu");
    std::fs::create_dir_all(&lib_dir)?;
    std::fs::copy(&tools.libext2fs, lib_dir.join("libext2fs.so.2.4"))?;
    std::fs::copy(&tools.libe2p, lib_dir.join("libe2p.so.2.3"))?;
    std::fs::copy(&tools.libcom_err, lib_dir.join("libcom_err.so.2.1"))?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("libext2fs.so.2.4", lib_dir.join("libext2fs.so.2"))?;
        std::os::unix::fs::symlink("libe2p.so.2.3", lib_dir.join("libe2p.so.2"))?;
        std::os::unix::fs::symlink("libcom_err.so.2.1", lib_dir.join("libcom_err.so.2"))?;
    }
    info!("Embedded virtio_blk/ext4 modules and e2fsprogs in initramfs");
    Ok(())
}

/// Shell snippet shared by both init variants: load disk modules, find the
/// virtio block device, format it on first boot, and mount it as the guest
/// state dir. All failures degrade to ephemeral state so the VM always boots.
const DISK_SETUP_SNIPPET: &str = r#"
# Persistent guest-state disk (virtio-blk, ext4).
KVER=$(ls /lib/modules 2>/dev/null | head -n 1)
if [ -n "$KVER" ]; then
    depmod -a "$KVER" 2>/dev/null
    modprobe virtio_blk 2>/dev/null || true
    modprobe ext4 2>/dev/null || true
fi
DISK=""
i=0
while [ "$i" -lt 10 ] && [ -z "$DISK" ]; do
    for dev in /dev/vda /dev/vdb; do
        if [ -b "$dev" ]; then DISK="$dev"; break; fi
    done
    [ -z "$DISK" ] && sleep 1
    i=$((i + 1))
done
if [ -n "$DISK" ]; then
    if ! blkid "$DISK" >/dev/null 2>&1; then
        echo "exo-vm-guest: formatting $DISK as ext4 for persistent state"
        mke2fs -q -t ext4 -L exo-state "$DISK" \
            || echo "exo-vm-guest: mke2fs failed; guest state will be ephemeral"
    fi
    mkdir -p /var/lib/exo-guest
    if mount -t ext4 "$DISK" /var/lib/exo-guest 2>/dev/null; then
        echo "exo-vm-guest: mounted $DISK at /var/lib/exo-guest"
    else
        echo "exo-vm-guest: WARNING could not mount $DISK; guest state will be ephemeral"
    fi
fi
"#;

fn write_init_script(temp_path: &Path) -> anyhow::Result<()> {
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

        init_script = format!(
            "#!/bin/sh\n\
             PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin\n\
             export PATH\n\
             mkdir -p /proc /sys /dev /tmp\n\
             mount -t proc proc /proc\n\
             mount -t sysfs sysfs /sys\n\
             mount -t devtmpfs devtmpfs /dev 2>/dev/null || true\n\
             {DISK_SETUP_SNIPPET}\n\
             exec /usr/local/bin/exo-vm-guest-init\n"
        )
        .into_bytes();
    } else {
        init_script = format!(
            r#"#!/bin/sh
# Exo microVM fallback init - prove the VM boots and keep it alive.
PATH=/usr/local/bin:/usr/local/sbin:/usr/bin:/usr/sbin:/bin:/sbin
export PATH
exec 1>/dev/console 2>/dev/console
echo "exo-vm-guest: shell fallback init started"
mkdir -p /proc /sys /dev /tmp
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
{DISK_SETUP_SNIPPET}
while true; do
    echo "exo-vm-guest: heartbeat"
    sleep 10
done
"#
        )
        .into_bytes();
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
    Ok(())
}

fn repack_initrd(temp_path: &Path, out: &Path) -> anyhow::Result<()> {
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

fn create_sparse_disk(path: &Path, size: u64) -> anyhow::Result<()> {
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

fn shell_escape(path: &Path) -> String {
    // Sufficient for paths that don't contain single quotes.
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}
