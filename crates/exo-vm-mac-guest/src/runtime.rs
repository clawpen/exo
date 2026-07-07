//! Minimal in-guest container runtime.
//!
//! This is intentionally small and self-contained so it can live in the
//! initramfs guest agent. It provides the lifecycle surface needed by the host
//! bridge while the full Exo Linux runtime is being integrated:
//!
//! - synchronous and detached command execution
//! - persisted container metadata
//! - stdout/stderr logs
//! - named guest volumes
//! - optional rootfs/chroot execution when a rootfs directory exists
//! - port mapping metadata records (actual host forwarding happens outside)
//!
//! It is not yet a full OCI runtime. Image names are mapped to rootfs
//! directories under `<state>/images/<image-ref>`, and the command runs either
//! inside that rootfs (Linux `chroot`) or directly in the guest when no rootfs
//! exists.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortSpec {
    pub host_ip: String,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSpec {
    pub mode: String,
    pub ports: Vec<PortSpec>,
    pub dns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub workdir: String,
    pub env: Vec<String>,
    pub mounts: Vec<MountSpec>,
    pub network: NetworkSpec,
    pub gpu: bool,
    pub memory_mb: Option<u64>,
    pub cpu: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub pid: Option<u32>,
    pub ports: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerRecord {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub workdir: String,
    pub env: Vec<String>,
    pub mounts: Vec<MountSpec>,
    pub network: NetworkSpec,
    pub status: String,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub created_at_ms: u128,
    pub started_at_ms: Option<u128>,
    pub stopped_at_ms: Option<u128>,
}

pub struct GuestRuntime {
    root: PathBuf,
}

pub struct RunOutcome {
    pub id: String,
    pub name: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayLayout {
    pub lower: PathBuf,
    pub upper: PathBuf,
    pub work: PathBuf,
    pub merged: PathBuf,
}

impl GuestRuntime {
    pub fn new_default() -> Result<Self> {
        Self::new(
            std::env::var("EXO_GUEST_STATE_DIR")
                .unwrap_or_else(|_| "/var/lib/exo-guest".to_string()),
        )
    }

    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let runtime = Self {
            root: root.as_ref().to_path_buf(),
        };
        runtime.ensure_layout()?;
        Ok(runtime)
    }

    fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(self.containers_dir())?;
        fs::create_dir_all(self.images_dir())?;
        fs::create_dir_all(self.volumes_dir())?;
        Ok(())
    }

    fn containers_dir(&self) -> PathBuf {
        self.root.join("containers")
    }

    fn images_dir(&self) -> PathBuf {
        self.root.join("images")
    }

    fn volumes_dir(&self) -> PathBuf {
        self.root.join("volumes")
    }

    fn container_dir(&self, name: &str) -> PathBuf {
        self.containers_dir().join(name)
    }

    fn record_path(&self, name: &str) -> PathBuf {
        self.container_dir(name).join("record.json")
    }

    fn stdout_path(&self, name: &str) -> PathBuf {
        self.container_dir(name).join("stdout.log")
    }

    fn stderr_path(&self, name: &str) -> PathBuf {
        self.container_dir(name).join("stderr.log")
    }

    pub fn run_container(&self, spec: ContainerSpec, detach: bool, rm: bool) -> Result<RunOutcome> {
        if spec.command.is_empty() {
            anyhow::bail!("no command specified");
        }
        let id = format!("guest-{}-{}", sanitize_name(&spec.name), now_ms());
        let mut record = ContainerRecord::new(id.clone(), spec.clone());
        record.status = "created".to_string();
        self.save_record(&record)?;

        if detach {
            return self.run_detached(record);
        }

        let (code, stdout, stderr) = self.run_sync(&spec)?;
        record.status = "exited".to_string();
        record.exit_code = Some(code);
        record.stopped_at_ms = Some(now_ms());
        self.append_logs(&record.name, &stdout, &stderr)?;
        self.save_record(&record)?;
        if rm {
            let _ = self.remove_container(&record.name, true);
        }
        Ok(RunOutcome {
            id,
            name: spec.name,
            exit_code: Some(code),
            stdout,
            stderr,
        })
    }

    fn run_detached(&self, mut record: ContainerRecord) -> Result<RunOutcome> {
        let stdout = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.stdout_path(&record.name))?;
        let stderr = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.stderr_path(&record.name))?;
        let mut cmd = self.command_for_spec(&record)?;
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::from(stdout));
        cmd.stderr(Stdio::from(stderr));
        let child = cmd.spawn()?;
        record.pid = Some(child.id());
        record.status = "running".to_string();
        record.started_at_ms = Some(now_ms());
        self.save_record(&record)?;
        std::mem::forget(child);
        Ok(RunOutcome {
            id: record.id,
            name: record.name,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn run_sync(&self, spec: &ContainerSpec) -> Result<(i32, String, String)> {
        let record = ContainerRecord::new("sync".to_string(), spec.clone());
        let output = self.command_for_spec(&record)?.output()?;
        let code = output
            .status
            .code()
            .unwrap_or_else(|| if output.status.success() { 0 } else { 1 });
        Ok((
            code,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }

    fn command_for_spec(&self, record: &ContainerRecord) -> Result<Command> {
        let spec = record.to_spec();
        self.prepare_volumes_and_mounts(&spec)?;

        let image_rootfs = self.rootfs_for_image(&spec.image);
        let runtime_rootfs = if image_rootfs.exists() {
            #[cfg(target_os = "linux")]
            {
                let layout = self.overlay_layout(&record.name, &spec.image);
                self.mount_overlay(&layout)?;
                layout.merged
            }
            #[cfg(not(target_os = "linux"))]
            {
                image_rootfs.clone()
            }
        } else {
            image_rootfs.clone()
        };

        let mut command = if runtime_rootfs.exists() {
            #[cfg(target_os = "linux")]
            self.apply_bind_mounts(&runtime_rootfs, &spec)?;
            command_in_rootfs(&runtime_rootfs, &spec)?
        } else {
            command_in_guest(&spec)?
        };

        for entry in &spec.env {
            if let Some((key, value)) = entry.split_once('=') {
                command.env(key, value);
            }
        }
        Ok(command)
    }

    fn prepare_volumes_and_mounts(&self, spec: &ContainerSpec) -> Result<()> {
        for mount in &spec.mounts {
            if is_named_volume_source(&mount.source) {
                fs::create_dir_all(self.volumes_dir().join(&mount.source))?;
            }
        }
        Ok(())
    }

    /// Resolve a mount source string to a guest path: named volumes are backed
    /// by the guest volume store; bind paths are used as-is.
    pub fn resolve_mount_source(&self, source: &str) -> PathBuf {
        if is_named_volume_source(source) {
            self.volumes_dir().join(source)
        } else {
            PathBuf::from(source)
        }
    }

    /// Build concrete mount plan tuples: `(source, target, readonly)`.
    pub fn mount_plan(&self, spec: &ContainerSpec) -> Vec<(PathBuf, String, bool)> {
        spec.mounts
            .iter()
            .map(|mount| {
                (
                    self.resolve_mount_source(&mount.source),
                    mount.target.clone(),
                    mount.readonly,
                )
            })
            .collect()
    }

    #[cfg(target_os = "linux")]
    fn apply_bind_mounts(&self, rootfs: &Path, spec: &ContainerSpec) -> Result<()> {
        use std::ffi::CString;
        for (source, target, readonly) in self.mount_plan(spec) {
            fs::create_dir_all(&source)?;
            let dest = rootfs.join(target.trim_start_matches('/'));
            fs::create_dir_all(&dest)?;
            let src_c = CString::new(source.to_string_lossy().as_bytes())?;
            let dst_c = CString::new(dest.to_string_lossy().as_bytes())?;
            let bind_c = CString::new("bind")?;
            let rc = unsafe {
                libc::mount(
                    src_c.as_ptr(),
                    dst_c.as_ptr(),
                    bind_c.as_ptr(),
                    libc::MS_BIND | libc::MS_REC,
                    std::ptr::null(),
                )
            };
            if rc != 0 {
                anyhow::bail!(
                    "bind mount {} -> {} failed: {}",
                    source.display(),
                    dest.display(),
                    std::io::Error::last_os_error()
                );
            }
            if readonly {
                let rc = unsafe {
                    libc::mount(
                        src_c.as_ptr(),
                        dst_c.as_ptr(),
                        bind_c.as_ptr(),
                        libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY,
                        std::ptr::null(),
                    )
                };
                if rc != 0 {
                    anyhow::bail!(
                        "readonly remount of {} failed: {}",
                        dest.display(),
                        std::io::Error::last_os_error()
                    );
                }
            }
        }
        Ok(())
    }

    fn rootfs_for_image(&self, image: &str) -> PathBuf {
        self.images_dir().join(sanitize_name(image)).join("rootfs")
    }

    /// Directory that holds the extracted rootfs for an image.
    pub fn image_rootfs_dir(&self, image: &str) -> PathBuf {
        self.rootfs_for_image(image)
    }

    /// Import an image rootfs from a `.tar` or `.tar.gz` archive into the guest
    /// image store. Returns the extracted rootfs directory.
    pub fn import_image_from_tar(&self, image: &str, tar_path: &Path) -> Result<PathBuf> {
        let dest = self.image_rootfs_dir(image);
        if dest.exists() {
            fs::remove_dir_all(&dest).ok();
        }
        fs::create_dir_all(&dest)?;
        let file = fs::File::open(tar_path)
            .with_context(|| format!("open image archive {}", tar_path.display()))?;
        if is_gzip(tar_path)? {
            let decoder = flate2::read::GzDecoder::new(file);
            tar::Archive::new(decoder)
                .unpack(&dest)
                .with_context(|| format!("extract {} to {}", tar_path.display(), dest.display()))?;
        } else {
            tar::Archive::new(file)
                .unpack(&dest)
                .with_context(|| format!("extract {} to {}", tar_path.display(), dest.display()))?;
        }
        Ok(dest)
    }

    /// Overlay layout for a container run: read-only image rootfs as the lower
    /// layer, per-container writable upper/work, and a merged mountpoint.
    pub fn overlay_layout(&self, container_name: &str, image: &str) -> OverlayLayout {
        let base = self.container_dir(container_name).join("overlay");
        OverlayLayout {
            lower: self.image_rootfs_dir(image),
            upper: base.join("upper"),
            work: base.join("work"),
            merged: base.join("merged"),
        }
    }

    #[cfg(target_os = "linux")]
    fn mount_overlay(&self, layout: &OverlayLayout) -> Result<()> {
        use std::ffi::CString;
        fs::create_dir_all(&layout.upper)?;
        fs::create_dir_all(&layout.work)?;
        fs::create_dir_all(&layout.merged)?;
        let opts = format!(
            "lowerdir={},upperdir={},workdir={}",
            layout.lower.display(),
            layout.upper.display(),
            layout.work.display()
        );
        let src = CString::new("overlay")?;
        let fstype = CString::new("overlay")?;
        let target = CString::new(layout.merged.to_string_lossy().as_bytes())?;
        let data = CString::new(opts)?;
        let rc = unsafe {
            libc::mount(
                src.as_ptr(),
                target.as_ptr(),
                fstype.as_ptr(),
                0,
                data.as_ptr() as *const libc::c_void,
            )
        };
        if rc != 0 {
            anyhow::bail!(
                "overlay mount at {} failed: {}",
                layout.merged.display(),
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    pub fn list_containers(&self, all: bool) -> Result<Vec<ContainerSummary>> {
        let mut out = vec![];
        for record in self.load_all_records()? {
            if !all && record.status != "running" {
                continue;
            }
            out.push(record.summary());
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn stop_container(&self, id: &str, force: bool) -> Result<()> {
        let mut record = self.find_record(id)?;
        if let Some(pid) = record.pid {
            #[cfg(unix)]
            unsafe {
                let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
                libc::kill(pid as i32, sig);
            }
        }
        record.status = "exited".to_string();
        record.stopped_at_ms = Some(now_ms());
        record.pid = None;
        self.save_record(&record)
    }

    pub fn remove_container(&self, id: &str, force: bool) -> Result<()> {
        let record = self.find_record(id)?;
        if record.status == "running" && !force {
            anyhow::bail!("container {} is running; use force", record.name);
        }
        if record.status == "running" {
            let _ = self.stop_container(&record.name, true);
        }
        fs::remove_dir_all(self.container_dir(&record.name)).or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            }
        })?;
        Ok(())
    }

    pub fn logs(&self, id: &str, tail: usize) -> Result<String> {
        let record = self.find_record(id)?;
        let mut combined = String::new();
        combined.push_str(&read_to_string_lossy(self.stdout_path(&record.name))?);
        combined.push_str(&read_to_string_lossy(self.stderr_path(&record.name))?);
        if tail == 0 {
            return Ok(combined);
        }
        let lines: Vec<_> = combined.lines().collect();
        let start = lines.len().saturating_sub(tail);
        Ok(lines[start..].join("\n") + if lines.is_empty() { "" } else { "\n" })
    }

    pub fn exec(&self, id: &str, command: Vec<String>) -> Result<(i32, String, String)> {
        let record = self.find_record(id)?;
        let mut spec = record.to_spec();
        spec.command = command;
        self.run_sync(&spec)
    }

    fn save_record(&self, record: &ContainerRecord) -> Result<()> {
        let dir = self.container_dir(&record.name);
        fs::create_dir_all(&dir)?;
        fs::write(
            self.record_path(&record.name),
            serde_json::to_vec_pretty(record)?,
        )?;
        Ok(())
    }

    fn load_record_by_name(&self, name: &str) -> Result<ContainerRecord> {
        let bytes = fs::read(self.record_path(name))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn load_all_records(&self) -> Result<Vec<ContainerRecord>> {
        let mut out = vec![];
        if !self.containers_dir().exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(self.containers_dir())? {
            let entry = entry?;
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let path = entry.path().join("record.json");
                if path.exists() {
                    let bytes = fs::read(path)?;
                    out.push(serde_json::from_slice(&bytes)?);
                }
            }
        }
        Ok(out)
    }

    fn find_record(&self, id_or_name: &str) -> Result<ContainerRecord> {
        if self.record_path(id_or_name).exists() {
            return self.load_record_by_name(id_or_name);
        }
        for record in self.load_all_records()? {
            if record.id.starts_with(id_or_name) || record.name == id_or_name {
                return Ok(record);
            }
        }
        anyhow::bail!("container not found: {}", id_or_name)
    }

    fn append_logs(&self, name: &str, stdout: &str, stderr: &str) -> Result<()> {
        fs::create_dir_all(self.container_dir(name))?;
        if !stdout.is_empty() {
            use std::io::Write;
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.stdout_path(name))?;
            file.write_all(stdout.as_bytes())?;
        }
        if !stderr.is_empty() {
            use std::io::Write;
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.stderr_path(name))?;
            file.write_all(stderr.as_bytes())?;
        }
        Ok(())
    }
}

impl ContainerRecord {
    fn new(id: String, spec: ContainerSpec) -> Self {
        Self {
            id,
            name: spec.name,
            image: spec.image,
            command: spec.command,
            workdir: spec.workdir,
            env: spec.env,
            mounts: spec.mounts,
            network: spec.network,
            status: "created".to_string(),
            pid: None,
            exit_code: None,
            created_at_ms: now_ms(),
            started_at_ms: None,
            stopped_at_ms: None,
        }
    }

    fn to_spec(&self) -> ContainerSpec {
        ContainerSpec {
            name: self.name.clone(),
            image: self.image.clone(),
            command: self.command.clone(),
            workdir: self.workdir.clone(),
            env: self.env.clone(),
            mounts: self.mounts.clone(),
            network: self.network.clone(),
            gpu: false,
            memory_mb: None,
            cpu: None,
        }
    }

    fn summary(&self) -> ContainerSummary {
        ContainerSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            image: self.image.clone(),
            status: self.status.clone(),
            pid: self.pid,
            ports: self
                .network
                .ports
                .iter()
                .map(|p| format!("{}:{}", p.host_port, p.container_port))
                .collect(),
        }
    }
}

fn command_in_guest(spec: &ContainerSpec) -> Result<Command> {
    let (program, args) = spec
        .command
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("no command specified"))?;
    let mut cmd = Command::new(program);
    cmd.args(args);
    let workdir = Path::new(&spec.workdir);
    if workdir.exists() {
        cmd.current_dir(workdir);
    }
    Ok(cmd)
}

fn command_in_rootfs(rootfs: &Path, spec: &ContainerSpec) -> Result<Command> {
    #[cfg(target_os = "linux")]
    {
        let (program, args) = spec
            .command
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("no command specified"))?;
        let mut cmd = Command::new("chroot");
        cmd.arg(rootfs);
        if !spec.workdir.is_empty() && spec.workdir != "/" {
            cmd.args(["/bin/sh", "-lc"]);
            let shell = format!(
                "cd {} && exec {}",
                shell_quote(&spec.workdir),
                std::iter::once(program.as_str())
                    .chain(args.iter().map(String::as_str))
                    .map(shell_quote)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            cmd.arg(shell);
        } else {
            cmd.arg(program);
            cmd.args(args);
        }
        Ok(cmd)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = rootfs;
        command_in_guest(spec)
    }
}

fn read_to_string_lossy(path: PathBuf) -> Result<String> {
    match fs::File::open(&path) {
        Ok(mut file) => {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("read log {}", path.display())),
    }
}

fn is_gzip(path: &Path) -> Result<bool> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "gz" || e == "tgz")
        .unwrap_or(false)
    {
        return Ok(true);
    }
    let mut file = fs::File::open(path)?;
    let mut magic = [0u8; 2];
    let n = file.read(&mut magic)?;
    let _ = file.seek(SeekFrom::Start(0));
    Ok(n == 2 && magic == [0x1f, 0x8b])
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_named_volume_source(value: &str) -> bool {
    !value.contains('/') && !value.is_empty()
}

#[cfg(target_os = "linux")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, command: Vec<&str>) -> ContainerSpec {
        ContainerSpec {
            name: name.to_string(),
            image: "guest".to_string(),
            command: command.into_iter().map(String::from).collect(),
            workdir: "/".to_string(),
            env: vec!["A=B".to_string()],
            mounts: vec![MountSpec {
                source: "data".to_string(),
                target: "/data".to_string(),
                readonly: false,
            }],
            network: NetworkSpec {
                mode: "bridge".to_string(),
                ports: vec![PortSpec {
                    host_ip: "127.0.0.1".to_string(),
                    host_port: 8080,
                    container_port: 80,
                    protocol: "tcp".to_string(),
                }],
                dns: vec![],
            },
            gpu: false,
            memory_mb: None,
            cpu: None,
        }
    }

    #[test]
    fn run_sync_persists_logs_and_record() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = GuestRuntime::new(dir.path()).unwrap();
        let out = runtime
            .run_container(
                spec("hello", vec!["sh", "-c", "printf %s \"$A\""]),
                false,
                false,
            )
            .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.stdout, "B");
        let listed = runtime.list_containers(true).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "hello");
        assert_eq!(listed[0].ports, vec!["8080:80".to_string()]);
        assert_eq!(runtime.logs("hello", 100).unwrap(), "B\n");
        assert!(runtime.volumes_dir().join("data").is_dir());
    }

    #[test]
    fn remove_container_deletes_record() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = GuestRuntime::new(dir.path()).unwrap();
        runtime
            .run_container(spec("gone", vec!["true"]), false, false)
            .unwrap();
        assert_eq!(runtime.list_containers(true).unwrap().len(), 1);
        runtime.remove_container("gone", true).unwrap();
        assert!(runtime.list_containers(true).unwrap().is_empty());
    }

    #[test]
    fn exec_uses_existing_record_context() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = GuestRuntime::new(dir.path()).unwrap();
        runtime
            .run_container(spec("ctx", vec!["true"]), false, false)
            .unwrap();
        let (code, stdout, _) = runtime
            .exec(
                "ctx",
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf %s \"$A\"".to_string(),
                ],
            )
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(stdout, "B");
    }

    #[test]
    fn mount_plan_resolves_named_volumes_and_bind_paths() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = GuestRuntime::new(dir.path()).unwrap();
        let mut s = spec("mounts", vec!["true"]);
        s.mounts.push(MountSpec {
            source: "/host/path".to_string(),
            target: "/host".to_string(),
            readonly: true,
        });
        let plan = runtime.mount_plan(&s);
        assert_eq!(plan[0].0, runtime.volumes_dir().join("data"));
        assert_eq!(plan[0].1, "/data");
        assert!(!plan[0].2);
        assert_eq!(plan[1].0, PathBuf::from("/host/path"));
        assert_eq!(plan[1].1, "/host");
        assert!(plan[1].2);
    }

    #[test]
    fn import_image_from_tar_extracts_rootfs() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = GuestRuntime::new(dir.path()).unwrap();
        let tar_path = dir.path().join("rootfs.tar");
        {
            let tar_file = fs::File::create(&tar_path).unwrap();
            let mut builder = tar::Builder::new(tar_file);
            let mut header = tar::Header::new_gnu();
            let content = b"hello";
            header.set_path("etc/message").unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &content[..]).unwrap();
            builder.finish().unwrap();
        }

        let rootfs = runtime
            .import_image_from_tar("alpine:local", &tar_path)
            .unwrap();
        assert_eq!(
            fs::read_to_string(rootfs.join("etc/message")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn overlay_layout_points_at_image_and_container_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = GuestRuntime::new(dir.path()).unwrap();
        let layout = runtime.overlay_layout("web", "alpine:local");
        assert_eq!(layout.lower, runtime.image_rootfs_dir("alpine:local"));
        assert!(layout.upper.ends_with("containers/web/overlay/upper"));
        assert!(layout.work.ends_with("containers/web/overlay/work"));
        assert!(layout.merged.ends_with("containers/web/overlay/merged"));
    }
}
