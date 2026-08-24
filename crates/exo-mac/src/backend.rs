//! Native process backend for macOS.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use exo_runtime::{ContainerConfig, RestartPolicy, SandboxMode};
use exo_runtime::{ContainerJson, ContainerListJson, ContainerManager, ContainerMetadata};
use exo_runtime::{MountConfig, NetworkConfig};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use crate::{detect_gpus, gpu_environment, MacConfig, MacGpuInfo, PathTranslator};

/// Options for `exo run` on macOS.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    pub detach: bool,
    pub rm: bool,
}

/// Options for `exo logs` on macOS.
#[derive(Debug, Clone, Copy)]
pub struct LogOptions {
    pub follow: bool,
    pub tail: usize,
    pub timestamps: bool,
}

/// Native macOS Exo backend.
pub struct NativeMacBackend {
    config: MacConfig,
    manager: ContainerManager,
    paths: PathTranslator,
}

impl NativeMacBackend {
    pub fn new(config: MacConfig) -> Result<Self> {
        Ok(Self {
            paths: PathTranslator::new(config.clone()),
            config,
            manager: ContainerManager::new()?,
        })
    }

    pub fn state_dir(&self) -> &Path {
        self.manager.state_dir()
    }

    pub fn run(&self, config: ContainerConfig, opts: RunOptions) -> Result<String> {
        let gpu = self.prepare_gpu(&config)?;
        self.warn_unsupported_features(&config);

        if self.manager.exists(&config.name) {
            anyhow::bail!("Container name '{}' is already in use", config.name);
        }

        let mut metadata = ContainerMetadata::new(config.name.clone(), config.clone());
        metadata
            .labels
            .insert("exo.backend".to_string(), self.config.backend_name.clone());
        metadata
            .labels
            .insert("exo.macos.mode".to_string(), "host-process".to_string());
        if let Some(gpu) = &gpu {
            metadata
                .labels
                .insert("exo.gpu.name".to_string(), gpu.name.clone());
            metadata.labels.insert(
                "exo.gpu.vendor".to_string(),
                gpu.vendor.as_str().to_string(),
            );
            metadata
                .labels
                .insert("exo.gpu.metal".to_string(), gpu.metal_supported.to_string());
        }
        metadata.ports = port_labels(&config.network);

        let container_dir = self.container_dir(&config.name);
        let logs_dir = container_dir.join("logs");
        fs::create_dir_all(&logs_dir)?;

        if opts.detach {
            let child =
                self.spawn_process(&config, ProcessIo::LogFiles(&logs_dir), gpu.as_ref())?;
            let pid = child.id();
            metadata.id = uuid::Uuid::new_v4().to_string();
            metadata.set_running(pid);
            self.manager.save(&metadata)?;
            write_child_pid(&container_dir, pid)?;
            std::mem::forget(child);
            return Ok(format!(
                "Container running in background: {}\nPID: {}\nName: {}\nBackend: native-macos\n",
                metadata.id, pid, metadata.name
            ));
        }

        let mut child = self.spawn_process(&config, ProcessIo::Inherit, gpu.as_ref())?;
        let pid = child.id();
        metadata.set_running(pid);
        if !opts.rm {
            self.manager.save(&metadata)?;
            write_child_pid(&container_dir, pid)?;
        }

        let status = child.wait()?;
        let code = status
            .code()
            .unwrap_or_else(|| if status.success() { 0 } else { 1 });
        metadata.set_stopped(Some(code));

        if opts.rm {
            let _ = self.manager.remove(&metadata.name);
        } else {
            self.manager.save(&metadata)?;
        }

        if code == 0 {
            Ok(String::new())
        } else {
            anyhow::bail!("Container exited with code {}", code)
        }
    }

    pub fn list(&self, all: bool) -> Result<Vec<ContainerMetadata>> {
        let mut containers = self.manager.list()?;
        for container in &mut containers {
            self.refresh_status(container)?;
        }

        if all {
            self.manager.list()
        } else {
            Ok(self
                .manager
                .list()?
                .into_iter()
                .filter(|c| c.is_running())
                .collect())
        }
    }

    pub fn list_json(&self, all: bool) -> Result<String> {
        let containers = self
            .list(all)?
            .into_iter()
            .map(ContainerJson::from)
            .collect();
        Ok(serde_json::to_string_pretty(&ContainerListJson {
            containers,
        })?)
    }

    pub fn start(&self, container: &str, attach: bool) -> Result<String> {
        let mut metadata = self.find(container)?;
        self.refresh_status(&mut metadata)?;
        if metadata.is_running() {
            return Ok(format!("Container {} is already running\n", metadata.name));
        }

        let logs_dir = self.container_dir(&metadata.name).join("logs");
        fs::create_dir_all(&logs_dir)?;
        let gpu = self.prepare_gpu(&metadata.config)?;
        let mut child = if attach {
            self.spawn_process(&metadata.config, ProcessIo::Inherit, gpu.as_ref())?
        } else {
            self.spawn_process(
                &metadata.config,
                ProcessIo::LogFiles(&logs_dir),
                gpu.as_ref(),
            )?
        };

        let pid = child.id();
        metadata.set_running(pid);
        self.manager.save(&metadata)?;
        write_child_pid(&self.container_dir(&metadata.name), pid)?;

        if attach {
            let status = child.wait()?;
            let code = status
                .code()
                .unwrap_or_else(|| if status.success() { 0 } else { 1 });
            metadata.set_stopped(Some(code));
            self.manager.save(&metadata)?;
            if code != 0 {
                anyhow::bail!("Container exited with code {}", code);
            }
            Ok(format!("Container {} exited with code 0\n", metadata.name))
        } else {
            std::mem::forget(child);
            Ok(format!(
                "Container {} started (PID: {})\n",
                metadata.name, pid
            ))
        }
    }

    pub fn stop(&self, container: &str, force: bool, timeout_secs: u64) -> Result<String> {
        let mut metadata = self.find(container)?;
        self.refresh_status(&mut metadata)?;
        if !metadata.is_running() {
            return Ok(format!(
                "Container {} is not running (status: {})\n",
                metadata.name, metadata.status
            ));
        }

        let pid = metadata
            .pid
            .ok_or_else(|| anyhow::anyhow!("Container has no PID"))?;
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        send_signal(pid, signal)?;

        if !force && !wait_for_exit(pid, Duration::from_secs(timeout_secs)) {
            send_signal(pid, libc::SIGKILL)?;
            let _ = wait_for_exit(pid, Duration::from_secs(1));
        }

        metadata.set_stopped(None);
        self.manager.save(&metadata)?;
        Ok(format!("Container {} stopped\n", metadata.name))
    }

    pub fn remove(&self, container: &str, force: bool) -> Result<String> {
        let mut metadata = self.find(container)?;
        self.refresh_status(&mut metadata)?;
        if metadata.is_running() && !force {
            anyhow::bail!(
                "Container {} is running. Use --force to stop and remove.",
                metadata.name
            );
        }

        if metadata.is_running() && force {
            let _ = self.stop(&metadata.name, true, 0);
        }

        self.manager.remove(&metadata.name)?;
        Ok(format!("Container {} removed\n", metadata.name))
    }

    pub fn logs(&self, container: &str, opts: LogOptions) -> Result<String> {
        let metadata = self.find(container)?;
        let logs_dir = self.container_dir(&metadata.name).join("logs");
        let stdout = logs_dir.join("stdout.log");
        let stderr = logs_dir.join("stderr.log");

        if opts.follow {
            self.follow_logs(&stdout, &stderr, opts.timestamps)?;
            return Ok(String::new());
        }

        let mut lines = vec![];
        lines.extend(read_log_lines(&stdout, opts.timestamps, "stdout")?);
        lines.extend(read_log_lines(&stderr, opts.timestamps, "stderr")?);

        let start = lines.len().saturating_sub(opts.tail);
        Ok(lines[start..].join(""))
    }

    pub fn exec(&self, container: &str, command: Vec<String>, user: Option<String>) -> Result<i32> {
        if command.is_empty() {
            anyhow::bail!("No command specified");
        }
        if user.is_some() {
            tracing::warn!("macOS native backend ignores --user; command runs as the current user");
        }

        let mut metadata = self.find(container)?;
        self.refresh_status(&mut metadata)?;
        if !metadata.is_running() {
            anyhow::bail!("Container {} is not running", metadata.name);
        }

        let mut exec_config = metadata.config.clone();
        exec_config.command = command;
        let gpu = self.prepare_gpu(&exec_config)?;
        let mut child = self.spawn_process(&exec_config, ProcessIo::Inherit, gpu.as_ref())?;
        let status = child.wait()?;
        Ok(status
            .code()
            .unwrap_or_else(|| if status.success() { 0 } else { 1 }))
    }

    pub fn refresh_status(&self, metadata: &mut ContainerMetadata) -> Result<()> {
        if let Some(pid) = metadata.pid {
            if process_is_alive(pid) {
                if metadata.status != "running" {
                    metadata.status = "running".to_string();
                    self.manager.save(metadata)?;
                }
            } else if metadata.status == "running" {
                metadata.set_stopped(None);
                self.manager.save(metadata)?;
            }
        }
        Ok(())
    }

    fn find(&self, container: &str) -> Result<ContainerMetadata> {
        self.manager
            .find(container)?
            .ok_or_else(|| exo_runtime::ExoError::ContainerNotFound(container.to_string()).into())
    }

    fn spawn_process(
        &self,
        config: &ContainerConfig,
        io: ProcessIo<'_>,
        gpu: Option<&MacGpuInfo>,
    ) -> Result<std::process::Child> {
        let (program, args) = command_parts(config);
        let workdir = self.resolve_workdir(config)?;
        let container_dir = self.container_dir(&config.name);
        let home_dir = container_dir.join("home");
        let tmp_dir = container_dir.join("tmp");
        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&tmp_dir)?;

        let sandbox_profile =
            self.write_sandbox_profile(config, &program, &workdir, &home_dir, &tmp_dir, &io)?;

        let mut cmd = if let Some(profile) = sandbox_profile {
            let mut cmd = Command::new("/usr/bin/sandbox-exec");
            cmd.arg("-f").arg(profile).arg(&program).args(&args);
            cmd
        } else {
            let mut cmd = Command::new(&program);
            cmd.args(&args);
            cmd
        };

        cmd.current_dir(workdir);
        self.apply_environment(&mut cmd, config, &home_dir, &tmp_dir, gpu)?;

        match io {
            ProcessIo::Inherit => {
                cmd.stdin(Stdio::inherit());
                cmd.stdout(Stdio::inherit());
                cmd.stderr(Stdio::inherit());
            }
            ProcessIo::LogFiles(logs_dir) => {
                fs::create_dir_all(logs_dir)?;
                let stdout = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(logs_dir.join("stdout.log"))?;
                let stderr = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(logs_dir.join("stderr.log"))?;
                cmd.stdin(Stdio::null());
                cmd.stdout(Stdio::from(stdout));
                cmd.stderr(Stdio::from(stderr));
            }
        }

        cmd.spawn().with_context(|| {
            format!(
                "failed to spawn macOS backend process for '{}'",
                config.name
            )
        })
    }

    fn apply_environment(
        &self,
        cmd: &mut Command,
        config: &ContainerConfig,
        home_dir: &Path,
        tmp_dir: &Path,
        gpu: Option<&MacGpuInfo>,
    ) -> Result<()> {
        cmd.env_clear();
        for (key, value) in self.build_environment(config, home_dir, tmp_dir, gpu)? {
            cmd.env(key, value);
        }
        Ok(())
    }

    fn build_environment(
        &self,
        config: &ContainerConfig,
        home_dir: &Path,
        tmp_dir: &Path,
        gpu: Option<&MacGpuInfo>,
    ) -> Result<HashMap<String, String>> {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), safe_path().to_string());
        env.insert("HOME".to_string(), home_dir.to_string_lossy().into_owned());
        env.insert("TMPDIR".to_string(), tmp_dir.to_string_lossy().into_owned());
        env.insert("TMP".to_string(), tmp_dir.to_string_lossy().into_owned());
        env.insert("TEMP".to_string(), tmp_dir.to_string_lossy().into_owned());
        env.insert("EXO_BACKEND".to_string(), self.config.backend_name.clone());
        env.insert(
            "EXO_MAC_SANDBOX".to_string(),
            (if sandbox_active(config) { "1" } else { "0" }).to_string(),
        );

        if let Some(gpu) = gpu {
            for (key, value) in gpu_environment(gpu) {
                env.insert(key, value);
            }
        }

        for (key, value) in &config.env {
            match key.as_str() {
                "HOME" | "TMPDIR" | "TMP" | "TEMP" => {
                    tracing::warn!(
                        "macOS backend ignores {} env override to preserve isolation",
                        key
                    );
                }
                _ => {
                    env.insert(key.clone(), value.clone());
                }
            }
        }

        // Inject requested secrets by name (values loaded from the secret
        // store at spawn time; never persisted in container metadata).
        if !config.secrets.is_empty() {
            let store = exo_runtime::SecretStore::new()?;
            for name in &config.secrets {
                match store.get(name)? {
                    Some(value) => {
                        env.insert(name.clone(), value);
                    }
                    None => {
                        return Err(exo_runtime::ExoError::SecretNotFound(format!(
                            "{name} (set it with 'exo secret set {name}')"
                        ))
                        .into());
                    }
                }
            }
        }

        Ok(env)
    }

    fn write_sandbox_profile(
        &self,
        config: &ContainerConfig,
        program: &str,
        workdir: &Path,
        home_dir: &Path,
        tmp_dir: &Path,
        io: &ProcessIo<'_>,
    ) -> Result<Option<PathBuf>> {
        match requested_sandbox_mode(config.sandbox) {
            SandboxMode::Off => return Ok(None),
            SandboxMode::Auto if !sandbox_available(true) => return Ok(None),
            SandboxMode::Required if !sandbox_available(false) => {
                anyhow::bail!(
                    "macOS sandbox was required but sandbox-exec is unavailable or preflight failed"
                );
            }
            SandboxMode::Auto | SandboxMode::Required => {}
        }

        let container_dir = self.container_dir(&config.name);
        let profile_path = container_dir.join("sandbox.sb");
        fs::create_dir_all(&container_dir)?;

        let SandboxAllowlists {
            read_paths,
            write_paths,
        } = self.sandbox_allowlists(config, program, workdir, home_dir, tmp_dir, io);

        let mut profile = String::from("(version 1)\n(deny default)\n");
        profile.push_str("(allow process*)\n");
        profile.push_str("(allow signal (target self))\n");
        profile.push_str("(allow sysctl*)\n");
        profile.push_str("(allow mach*)\n");
        profile.push_str("(allow ipc*)\n");
        if config.network.mode != "none" {
            profile.push_str("(allow network*)\n");
        }
        profile.push_str("(allow file-read-metadata)\n");
        profile.push_str("(allow file-read* (literal \"/dev/null\") (subpath \"/dev\"))\n");
        profile.push_str("(allow file-write* (literal \"/dev/null\") (subpath \"/dev\"))\n");
        profile.push_str("(allow file-read*");
        for path in dedup_paths(read_paths) {
            profile.push_str(&format!(" (subpath {})", scheme_quote(&path)));
        }
        profile.push_str(")\n");
        profile.push_str("(allow file-write*");
        for path in dedup_paths(write_paths) {
            profile.push_str(&format!(" (subpath {})", scheme_quote(&path)));
        }
        profile.push_str(")\n");

        fs::write(&profile_path, profile)?;
        Ok(Some(profile_path))
    }

    fn sandbox_allowlists(
        &self,
        config: &ContainerConfig,
        program: &str,
        workdir: &Path,
        home_dir: &Path,
        tmp_dir: &Path,
        io: &ProcessIo<'_>,
    ) -> SandboxAllowlists {
        let container_dir = self.container_dir(&config.name);
        let mut read_paths = default_read_paths();
        let mut write_paths = vec![
            container_dir.clone(),
            home_dir.to_path_buf(),
            tmp_dir.to_path_buf(),
        ];

        read_paths.push(container_dir.clone());
        read_paths.push(home_dir.to_path_buf());
        read_paths.push(tmp_dir.to_path_buf());
        read_paths.push(workdir.to_path_buf());

        for mount in &config.mounts {
            if let Ok(source) = self.mount_source_to_host(mount) {
                read_paths.push(source.clone());
                if !mount.readonly {
                    write_paths.push(source);
                }
            }
        }

        if let ProcessIo::LogFiles(path) = io {
            read_paths.push((*path).to_path_buf());
            write_paths.push((*path).to_path_buf());
        }

        let program_path = Path::new(program);
        if program_path.is_absolute() {
            if let Some(parent) = program_path.parent() {
                read_paths.push(parent.to_path_buf());
            }
        }

        SandboxAllowlists {
            read_paths,
            write_paths,
        }
    }

    fn resolve_workdir(&self, config: &ContainerConfig) -> Result<PathBuf> {
        if config.workdir.as_os_str().is_empty() || config.workdir == Path::new("/") {
            return Ok(std::env::current_dir()?);
        }

        if config.workdir.exists() {
            return Ok(self.paths.normalize_host_path(&config.workdir));
        }

        if let Some(mount) = mount_for_target(&config.mounts, &config.workdir) {
            let source = self.mount_source_to_host(mount)?;
            if source.exists() {
                return Ok(source);
            }
        }

        tracing::warn!(
            "workdir '{}' does not exist on macOS host; using current directory",
            config.workdir.display()
        );
        Ok(std::env::current_dir()?)
    }

    fn mount_source_to_host(&self, mount: &MountConfig) -> Result<PathBuf> {
        if mount.mount_type == "volume" {
            return Ok(exo_runtime::VolumeStore::new()?.ensure(&mount.source)?);
        }
        self.paths.mount_source_to_host(&mount.source)
    }

    fn container_dir(&self, name: &str) -> PathBuf {
        self.manager.state_dir().join(name)
    }

    fn prepare_gpu(&self, config: &ContainerConfig) -> Result<Option<MacGpuInfo>> {
        if config.gpu.is_none() {
            return Ok(None);
        }

        let gpus = detect_gpus()?;
        let Some(gpu) = select_gpu(&gpus, config.gpu.as_ref().unwrap()) else {
            anyhow::bail!("GPU requested but no compatible macOS GPU was detected");
        };

        tracing::info!(
            "Using macOS GPU: {} ({}, Metal: {})",
            gpu.name,
            gpu.vendor.as_str(),
            gpu.metal_supported
        );
        Ok(Some(gpu.clone()))
    }

    fn warn_unsupported_features(&self, config: &ContainerConfig) {
        if config.resources.memory.is_some()
            || config.resources.cpu.is_some()
            || config.resources.cpu_shares.is_some()
            || config.resources.pids_limit.is_some()
        {
            tracing::warn!("macOS native backend does not enforce Linux cgroup resource limits");
        }
        if config.network.mode != "host" && config.network.mode != "bridge" {
            tracing::warn!(
                "macOS native backend runs on the host network; requested network mode '{}' is not isolated",
                config.network.mode
            );
        }
        if config.privileged
            || config.readonly_rootfs
            || config.restart_policy != RestartPolicy::Never
        {
            tracing::warn!(
                "some Linux container options are not enforced by the macOS native backend"
            );
        }
    }

    fn follow_logs(&self, stdout: &Path, stderr: &Path, timestamps: bool) -> Result<()> {
        let mut out_offset = print_existing(stdout, timestamps, "stdout")?;
        let mut err_offset = print_existing(stderr, timestamps, "stderr")?;
        loop {
            out_offset = print_new(stdout, out_offset, timestamps, "stdout")?;
            err_offset = print_new(stderr, err_offset, timestamps, "stderr")?;
            thread::sleep(Duration::from_millis(500));
        }
    }
}

#[async_trait]
impl exo_runtime::ExoBackend for NativeMacBackend {
    async fn run(
        &self,
        config: ContainerConfig,
        opts: exo_runtime::BackendRunOptions,
    ) -> Result<exo_runtime::RunResult> {
        let name = config.name.clone();
        let output = NativeMacBackend::run(
            self,
            config,
            RunOptions {
                detach: opts.detach,
                rm: opts.rm,
            },
        )?;
        Ok(exo_runtime::RunResult {
            id: None,
            name,
            message: output,
            exit_code: Some(0),
        })
    }

    async fn list(
        &self,
        opts: exo_runtime::ListOptions,
    ) -> Result<Vec<exo_runtime::ContainerMetadata>> {
        NativeMacBackend::list(self, opts.all)
    }

    async fn start(&self, id: &str, opts: exo_runtime::StartOptions) -> Result<()> {
        NativeMacBackend::start(self, id, opts.attach).map(|_| ())
    }

    async fn stop(&self, id: &str, opts: exo_runtime::StopOptions) -> Result<()> {
        NativeMacBackend::stop(self, id, opts.force, opts.timeout_secs).map(|_| ())
    }

    async fn remove(&self, id: &str, opts: exo_runtime::RemoveOptions) -> Result<()> {
        NativeMacBackend::remove(self, id, opts.force).map(|_| ())
    }

    async fn logs(
        &self,
        id: &str,
        opts: exo_runtime::BackendLogOptions,
    ) -> Result<exo_runtime::LogStream> {
        let content = NativeMacBackend::logs(
            self,
            id,
            LogOptions {
                follow: opts.follow,
                tail: opts.tail,
                timestamps: opts.timestamps,
            },
        )?;
        Ok(exo_runtime::LogStream { content })
    }

    async fn exec(
        &self,
        id: &str,
        command: Vec<String>,
        opts: exo_runtime::ExecOptions,
    ) -> Result<i32> {
        NativeMacBackend::exec(self, id, command, opts.user)
    }

    fn capabilities(&self) -> exo_runtime::BackendCapabilities {
        exo_runtime::BackendCapabilities::native_macos()
    }
}

fn command_parts(config: &ContainerConfig) -> (String, Vec<String>) {
    if let Some((program, args)) = config.command.split_first() {
        (program.clone(), args.to_vec())
    } else {
        (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
            vec![],
        )
    }
}

fn select_gpu<'a>(
    gpus: &'a [MacGpuInfo],
    requested: &exo_runtime::config::GpuConfig,
) -> Option<&'a MacGpuInfo> {
    if gpus.is_empty() {
        return None;
    }

    let requested_type = requested.gpu_type.to_lowercase();
    match requested_type.as_str() {
        "auto" | "" => gpus
            .iter()
            .find(|g| g.metal_supported)
            .or_else(|| gpus.first()),
        "apple" | "mps" | "metal" => gpus
            .iter()
            .find(|g| g.vendor == crate::MacGpuVendor::Apple || g.metal_supported),
        "amd" => gpus.iter().find(|g| g.vendor == crate::MacGpuVendor::Amd),
        "nvidia" => gpus
            .iter()
            .find(|g| g.vendor == crate::MacGpuVendor::Nvidia),
        "intel" => gpus.iter().find(|g| g.vendor == crate::MacGpuVendor::Intel),
        _ => gpus
            .iter()
            .find(|g| g.metal_supported)
            .or_else(|| gpus.first()),
    }
}

fn requested_sandbox_mode(config_mode: SandboxMode) -> SandboxMode {
    if config_mode != SandboxMode::Auto {
        return config_mode;
    }
    std::env::var("EXO_MAC_SANDBOX")
        .ok()
        .and_then(|v| v.parse::<SandboxMode>().ok())
        .unwrap_or(SandboxMode::Auto)
}

fn sandbox_active(config: &ContainerConfig) -> bool {
    match requested_sandbox_mode(config.sandbox) {
        SandboxMode::Off => false,
        SandboxMode::Auto | SandboxMode::Required => sandbox_available(true),
    }
}

fn sandbox_available(warn: bool) -> bool {
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        if warn {
            tracing::warn!("macOS sandbox-exec is unavailable; using env isolation only");
        }
        return false;
    }
    sandbox_preflight(warn)
}

fn sandbox_preflight(warn: bool) -> bool {
    static CAN_SANDBOX: OnceLock<bool> = OnceLock::new();
    let available = *CAN_SANDBOX.get_or_init(|| {
        let status = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .status();
        match status {
            Ok(status) if status.success() => true,
            Ok(status) => {
                tracing::warn!(
                    "macOS sandbox-exec is unavailable in this environment (exit {:?}); using env isolation only",
                    status.code()
                );
                false
            }
            Err(e) => {
                tracing::warn!(
                    "macOS sandbox-exec preflight failed: {}; using env isolation only",
                    e
                );
                false
            }
        }
    });
    if warn && !available {
        tracing::warn!(
            "macOS sandbox-exec is unavailable in this environment; using env isolation only"
        );
    }
    available
}

fn safe_path() -> &'static str {
    "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"
}

fn default_read_paths() -> Vec<PathBuf> {
    [
        "/bin",
        "/sbin",
        "/usr",
        "/System",
        "/Library",
        "/opt/homebrew",
        "/private/etc",
        "/private/var/db",
        "/private/var/folders",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for path in paths {
        let path = path.canonicalize().unwrap_or(path);
        if !out.iter().any(|p| p == &path) {
            out.push(path);
        }
    }
    out
}

fn scheme_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

#[cfg_attr(not(test), allow(dead_code))]
fn mount_source_for_target(mounts: &[MountConfig], target: &Path) -> Option<String> {
    mount_for_target(mounts, target).map(|m| m.source.clone())
}

fn mount_for_target<'a>(mounts: &'a [MountConfig], target: &Path) -> Option<&'a MountConfig> {
    let target = target.to_string_lossy();
    mounts.iter().find(|m| m.target == target)
}

fn port_labels(network: &NetworkConfig) -> Vec<String> {
    network
        .port_mappings
        .iter()
        .map(|p| format!("{}:{}", p.host_port, p.container_port))
        .collect()
}

fn write_child_pid(container_dir: &Path, pid: u32) -> Result<()> {
    fs::create_dir_all(container_dir)?;
    fs::write(container_dir.join("pid"), pid.to_string())?;
    Ok(())
}

fn process_is_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn send_signal(pid: u32, signal: i32) -> Result<()> {
    let rc = unsafe { libc::kill(pid as i32, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to signal pid {}", pid))
    }
}

fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !process_is_alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    !process_is_alive(pid)
}

fn read_log_lines(path: &Path, timestamps: bool, stream: &str) -> Result<Vec<String>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    Ok(content
        .lines()
        .map(|line| format_log_line(line, timestamps, stream))
        .collect())
}

fn print_existing(path: &Path, timestamps: bool, stream: &str) -> Result<u64> {
    let lines = read_log_lines(path, timestamps, stream)?;
    for line in lines {
        print!("{}", line);
    }
    Ok(fs::metadata(path).map(|m| m.len()).unwrap_or(0))
}

fn print_new(path: &Path, offset: u64, timestamps: bool, stream: &str) -> Result<u64> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(offset),
        Err(e) => return Err(e.into()),
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    for line in buf.lines() {
        print!("{}", format_log_line(line, timestamps, stream));
    }
    Ok(file.metadata()?.len())
}

fn format_log_line(line: &str, timestamps: bool, stream: &str) -> String {
    if timestamps {
        format!("{} {} {}\n", Utc::now().to_rfc3339(), stream, line)
    } else {
        format!("{}\n", line)
    }
}

enum ProcessIo<'a> {
    Inherit,
    LogFiles(&'a Path),
}

struct SandboxAllowlists {
    read_paths: Vec<PathBuf>,
    write_paths: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use exo_runtime::config::{ContainerConfig, MountConfig};

    #[test]
    fn command_parts_uses_config_command() {
        let cfg = ContainerConfig {
            command: vec!["echo".to_string(), "hi".to_string()],
            ..Default::default()
        };
        let (program, args) = command_parts(&cfg);
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["hi".to_string()]);
    }

    #[test]
    fn mount_source_matching_target() {
        let source = mount_source_for_target(
            &[MountConfig {
                source: "/Users/me/app".to_string(),
                target: "/app".to_string(),
                readonly: false,
                mount_type: "bind".to_string(),
                size: None,
                propagation: "rprivate".to_string(),
            }],
            Path::new("/app"),
        );
        assert_eq!(source.as_deref(), Some("/Users/me/app"));
    }

    #[test]
    fn environment_is_secret_isolated_and_honors_explicit_env() {
        let state = tempfile::tempdir().unwrap();
        let backend = NativeMacBackend {
            config: crate::MacConfig::default(),
            manager: ContainerManager::with_state_dir(state.path()).unwrap(),
            paths: crate::PathTranslator::default(),
        };

        let mut cfg = ContainerConfig::default();
        cfg.env
            .insert("EXPLICIT_TOKEN".to_string(), "allowed".to_string());
        cfg.env
            .insert("HOME".to_string(), "/Users/real-user".to_string());
        cfg.env
            .insert("TMPDIR".to_string(), "/tmp/real".to_string());

        let env = backend
            .build_environment(
                &cfg,
                Path::new("/exo/state/home"),
                Path::new("/exo/state/tmp"),
                None,
            )
            .unwrap();

        assert_eq!(
            env.get("EXPLICIT_TOKEN").map(String::as_str),
            Some("allowed")
        );
        assert_eq!(env.get("HOME").map(String::as_str), Some("/exo/state/home"));
        assert_eq!(
            env.get("TMPDIR").map(String::as_str),
            Some("/exo/state/tmp")
        );
        assert!(!env.contains_key("SSH_AUTH_SOCK"));
        assert!(!env.contains_key("AWS_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn environment_injects_requested_secrets() {
        let state = tempfile::tempdir().unwrap();
        let secrets = tempfile::tempdir().unwrap();
        std::env::set_var("EXO_SECRETS_DIR", secrets.path());
        let store = exo_runtime::SecretStore::new().unwrap();
        store.set("API_TOKEN", "stored-value").unwrap();

        let backend = NativeMacBackend {
            config: crate::MacConfig::default(),
            manager: ContainerManager::with_state_dir(state.path()).unwrap(),
            paths: crate::PathTranslator::default(),
        };

        let cfg = ContainerConfig {
            secrets: vec!["API_TOKEN".to_string()],
            ..Default::default()
        };
        let env = backend
            .build_environment(
                &cfg,
                Path::new("/exo/state/home"),
                Path::new("/exo/state/tmp"),
                None,
            )
            .unwrap();

        assert_eq!(
            env.get("API_TOKEN").map(String::as_str),
            Some("stored-value")
        );
        std::env::remove_var("EXO_SECRETS_DIR");
    }

    #[test]
    fn sandbox_profile_paths_are_quoted() {
        let quoted = scheme_quote(Path::new("/Users/me/My \"Project\""));
        assert_eq!(quoted, "\"/Users/me/My \\\"Project\\\"\"");
    }

    #[test]
    fn sandbox_allowlist_respects_readonly_mounts() {
        let state = tempfile::tempdir().unwrap();
        let ro = tempfile::tempdir().unwrap();
        let rw = tempfile::tempdir().unwrap();
        let backend = NativeMacBackend {
            config: crate::MacConfig::default(),
            manager: ContainerManager::with_state_dir(state.path()).unwrap(),
            paths: crate::PathTranslator::default(),
        };

        let cfg = ContainerConfig {
            name: "allowlist-test".to_string(),
            mounts: vec![
                MountConfig {
                    source: ro.path().to_string_lossy().into_owned(),
                    target: "/ro".to_string(),
                    readonly: true,
                    mount_type: "bind".to_string(),
                    size: None,
                    propagation: "rprivate".to_string(),
                },
                MountConfig {
                    source: rw.path().to_string_lossy().into_owned(),
                    target: "/rw".to_string(),
                    readonly: false,
                    mount_type: "bind".to_string(),
                    size: None,
                    propagation: "rprivate".to_string(),
                },
            ],
            ..Default::default()
        };

        let allowlists = backend.sandbox_allowlists(
            &cfg,
            "/bin/echo",
            Path::new("/tmp"),
            state.path().join("home").as_path(),
            state.path().join("tmp").as_path(),
            &ProcessIo::Inherit,
        );

        let ro_path = ro.path().canonicalize().unwrap();
        let rw_path = rw.path().canonicalize().unwrap();

        assert!(allowlists.read_paths.iter().any(|p| p == &ro_path));
        assert!(allowlists.read_paths.iter().any(|p| p == &rw_path));
        assert!(!allowlists.write_paths.iter().any(|p| p == &ro_path));
        assert!(allowlists.write_paths.iter().any(|p| p == &rw_path));
    }
}
