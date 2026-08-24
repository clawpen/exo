//! macOS Linux microVM backend facade.
//!
//! This backend is the host-side contract for Docker/Podman-style Linux
//! containers on macOS. It intentionally exists before the VM bridge is fully
//! persistent so the CLI can route `--backend linux` through one place and the
//! remaining work is narrowed to connecting this facade to a live guest agent.

use crate::bridge::{ContainerSpec, GuestRequest, GuestResponse, MountSpec, NetworkSpec, PortSpec};
use async_trait::async_trait;
use exo_runtime::{
    BackendCapabilities, BackendLogOptions, BackendRunOptions, ContainerConfig, ContainerMetadata,
    ExecOptions, ExoBackend, ListOptions, LogStream, RemoveOptions, RunResult, StartOptions,
    StopOptions,
};

/// Host-side backend for Linux containers inside the Exo-managed macOS microVM.
#[derive(Debug, Clone)]
pub struct MacLinuxBackend {
    config: crate::VmConfig,
}

impl MacLinuxBackend {
    pub fn new(config: crate::VmConfig) -> Self {
        Self { config }
    }

    /// Convert shared `ContainerConfig` into the guest RPC run spec.
    pub fn container_spec(config: &ContainerConfig) -> ContainerSpec {
        ContainerSpec {
            name: config.name.clone(),
            image: config.image.clone(),
            command: config.command.clone(),
            workdir: config.workdir.to_string_lossy().to_string(),
            workspace: config.workspace.as_ref().map(|p| p.to_string_lossy().to_string()),
            env: config
                .env
                .iter()
                .map(|(key, value)| format!("{}={}", key, value))
                .collect(),
            mounts: config
                .mounts
                .iter()
                .map(|mount| MountSpec {
                    source: mount.source.clone(),
                    target: mount.target.clone(),
                    readonly: mount.readonly,
                })
                .collect(),
            network: NetworkSpec {
                mode: config.network.mode.clone(),
                ports: config
                    .network
                    .port_mappings
                    .iter()
                    .map(|port| PortSpec {
                        host_ip: port.host_ip.clone(),
                        host_port: port.host_port,
                        container_port: port.container_port,
                        protocol: port.protocol.clone(),
                    })
                    .collect(),
                dns: config.network.dns.clone(),
            },
            gpu: config.gpu.is_some(),
            memory_mb: config.resources.memory.as_deref().and_then(parse_memory_mb),
            cpu: config.resources.cpu.clone(),
        }
    }

    fn not_ready(&self) -> anyhow::Error {
        anyhow::anyhow!(
            "macOS Linux container backend is not ready: no persistent Exo microVM guest bridge is live for '{}'. Run 'exo vm init' and 'exo vm start', then retry. For current macOS use, pass '--backend native host'.",
            self.config.name
        )
    }

    /// Lazily bring up the microVM: initialize the VM image on first use,
    /// auto-start the control daemon when it is not running, and wait for the
    /// guest agent to answer. Idempotent and safe to call concurrently from
    /// multiple processes.
    pub async fn ensure_ready(&self) -> anyhow::Result<()> {
        use std::time::{Duration, Instant};

        if self.client().is_ok() {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        crate::ensure_virtualization_entitlement()?;

        // First-time setup: download/build the VM image (idempotent, cached).
        crate::builder::ensure_image(&self.config, false)
            .await
            .map_err(|e| anyhow::anyhow!("failed to initialize the Exo microVM image: {}", e))?;

        self.spawn_daemon_if_needed()?;

        // Wait for the VM to boot and the guest agent to answer.
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match self.client() {
                Ok(_) => return Ok(()),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(anyhow::anyhow!(
                            "timed out waiting for the Exo microVM guest agent: {}. See {}",
                            e,
                            crate::paths::daemon_log_path()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default()
                        ));
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    /// Start the control daemon when no live daemon owns the socket. Guards
    /// against concurrent auto-starts from parallel exo/Orchestre processes:
    /// the first process to create the lock file spawns the daemon; the rest
    /// wait for the socket to come up.
    fn spawn_daemon_if_needed(&self) -> anyhow::Result<()> {
        use std::fs::OpenOptions;
        use std::time::{Duration, Instant};

        let client = crate::VmDaemonClient::new()?;
        if client.is_running() {
            return Ok(());
        }

        let lock_path = crate::paths::exo_vm_dir()?.join("daemon-autostart.lock");
        let spawned = match OpenOptions::new().write(true).create_new(true).open(&lock_path) {
            Ok(_) => match crate::daemon::spawn_detached() {
                Ok(pid) => {
                    tracing::info!("Auto-started Exo VM daemon (PID: {})", pid);
                    true
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&lock_path);
                    return Err(e);
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Another process is starting the daemon; wait below. A stale
                // lock from a crashed spawner surfaces as a wait timeout with
                // a pointer to the daemon log, which is acceptable.
                tracing::info!("Another process is starting the Exo VM daemon; waiting");
                false
            }
            Err(e) => return Err(e.into()),
        };

        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if client.is_running() {
                if spawned {
                    let _ = std::fs::remove_file(&lock_path);
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if spawned {
            let _ = std::fs::remove_file(&lock_path);
        }
        anyhow::bail!(
            "timed out waiting for the Exo VM daemon to come up; see {}",
            crate::paths::daemon_log_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )
    }

    fn client(&self) -> anyhow::Result<crate::VmDaemonClient> {
        let client = crate::VmDaemonClient::new()?;
        if !client.is_running() {
            return Err(self.not_ready());
        }
        match client.status()? {
            crate::VmDaemonResponse::Status {
                running: true,
                guest_agent_reachable: true,
                ..
            } => Ok(client),
            crate::VmDaemonResponse::Status {
                guest_agent_info, ..
            } => anyhow::bail!(
                "EXO macOS Linux VM daemon is running, but the guest agent is not ready: {}",
                guest_agent_info
            ),
            other => anyhow::bail!("unexpected EXO VM daemon status: {:?}", other),
        }
    }

    /// Ask the live guest agent to import a rootfs tarball that is already
    /// visible at `guest_tar_path` inside the VM.
    pub fn import_image_from_guest_path(
        &self,
        image: &str,
        guest_tar_path: &str,
    ) -> anyhow::Result<String> {
        match self.client()?.guest_request(GuestRequest::ImportImage {
            image: image.to_string(),
            tar_path: guest_tar_path.to_string(),
        })? {
            GuestResponse::ImageImported { rootfs_path, .. } => Ok(rootfs_path),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to ImportImage: {:?}", other),
        }
    }

    /// Rootfs tarball URL for images we know how to auto-provision. Others return
    /// `None` and fall back to raw guest exec.
    fn image_rootfs_url(image: &str) -> Option<&'static str> {
        match image {
            "alpine" | "alpine:latest" | "alpine:3" | "alpine:3.20" => Some(
                "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/aarch64/alpine-minirootfs-3.20.3-aarch64.tar.gz",
            ),
            _ => None,
        }
    }

    /// Ensure an image rootfs exists in the guest. The guest has no TLS/network
    /// downloader, so the host fetches the tarball (cached) and streams it in over
    /// the RPC channel in hex-encoded chunks, then asks the guest to extract it.
    /// No-op for images we don't know how to provision or that are already present.
    pub async fn ensure_image(&self, image: &str) -> anyhow::Result<()> {
        self.ensure_ready().await?;
        let image_status = self.client()?.guest_request(GuestRequest::ImageExists {
            image: image.to_string(),
        })?;
        if matches!(
            image_status,
            GuestResponse::Ok { ref message } if message == "present"
        ) {
            return Ok(());
        }

        if !matches!(image_status, GuestResponse::Ok { .. }) {
            anyhow::bail!(
                "unexpected guest response while checking image: {:?}",
                image_status
            );
        }

        let safe: String = image
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let cache = crate::paths::exo_vm_dir()?.join("images");
        std::fs::create_dir_all(&cache)?;
        let host_tar = cache.join(format!("{}.tar.gz", safe));

        if !host_tar.exists() {
            if let Some(url) = Self::image_rootfs_url(image) {
                crate::image::download_file_if_missing(url, &host_tar).await?;
            } else {
                // Generic path: pull from the OCI registry and compose the
                // rootfs on the host.
                crate::oci::pull_rootfs_tar(image, &host_tar)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("failed to pull image '{}': {:#}", image, e)
                    })?;
            }
        }

        let bytes = std::fs::read(&host_tar)?;
        let guest_tar = format!("/tmp/exo-image-{}.tar.gz", safe);
        // Keep each hex-encoded line well under the serial socketpair's send
        // buffer (~8 KiB on macOS) so a WriteChunk never blocks past the RPC
        // timeout. 2 KiB raw -> ~4 KiB hex.
        const CHUNK: usize = 2 * 1024;
        let client = self.client()?;
        tracing::info!(
            "Provisioning image '{}' into guest ({} bytes, {} chunks)",
            image,
            bytes.len(),
            bytes.len().div_ceil(CHUNK)
        );
        for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
            match client.guest_request(GuestRequest::WriteChunk {
                path: guest_tar.clone(),
                data_hex: to_hex(chunk),
                append: i > 0,
            })? {
                GuestResponse::Ok { .. } => {}
                GuestResponse::Error { message } => anyhow::bail!("WriteChunk failed: {}", message),
                other => anyhow::bail!("unexpected response to WriteChunk: {:?}", other),
            }
        }
        self.import_image_from_guest_path(image, &guest_tar)?;
        tracing::info!("Image '{}' provisioned into guest", image);
        Ok(())
    }

    /// Remove an image rootfs from the guest store, e.g. to recover from a
    /// partial import or to force a re-provision on the next run.
    pub async fn remove_image(&self, image: &str) -> anyhow::Result<()> {
        self.ensure_ready().await?;
        match self.client()?.guest_request(GuestRequest::RemoveImage {
            image: image.to_string(),
        })? {
            GuestResponse::Ok { .. } => Ok(()),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to RemoveImage: {:?}", other),
        }
    }

    fn validate_run_config(config: &ContainerConfig) -> anyhow::Result<()> {
        // The guest brings up eth0 over the host's VZ NAT attachment; chrooted
        // containers share that namespace, so "nat" gives outbound networking.
        // TCP port publishing works through the vsock tunnel: the daemon binds
        // 127.0.0.1:<host_port> and forwards into the guest.
        if !matches!(config.network.mode.as_str(), "none" | "nat" | "bridge") {
            anyhow::bail!(
                "network mode '{}' is not implemented for the EXO macOS Linux VM yet; use --network nat (outbound) or --network none",
                config.network.mode
            );
        }
        for port in &config.network.port_mappings {
            if !port.protocol.eq_ignore_ascii_case("tcp") {
                anyhow::bail!(
                    "only TCP port publishing is supported by the EXO macOS Linux VM vsock tunnel, got '{}'",
                    port.protocol
                );
            }
        }
        if config.resources.memory.is_some()
            || config.resources.cpu.is_some()
            || config.resources.cpus.is_some()
            || config.resources.memory_swap.is_some()
            || config.resources.memory_reservation.is_some()
            || config.resources.cpu_shares.is_some()
            || config.resources.pids_limit.is_some()
        {
            return Err(exo_runtime::ExoError::BackendUnsupported {
                feature: "resource limits".to_string(),
                backend: "macos-linux-vm".to_string(),
            }
            .into());
        }
        if config.gpu.is_some() {
            return Err(exo_runtime::ExoError::BackendUnsupported {
                feature: "GPU passthrough".to_string(),
                backend: "macos-linux-vm".to_string(),
            }
            .into());
        }
        for mount in &config.mounts {
            if mount.source.contains('/') {
                return Err(exo_runtime::ExoError::BackendUnsupported {
                    feature: format!("host bind mount '{}' (use a named guest volume until virtio-fs lands)", mount.source),
                    backend: "macos-linux-vm".to_string(),
                }
                .into());
            }
        }
        Ok(())
    }
}

/// Lowercase hex-encode bytes for line-delimited JSON transport.
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// Create a gzipped tarball of a directory's *contents* and stream it into the
/// guest via WriteChunk requests.
fn push_workspace_to_guest(
    backend: &MacLinuxBackend,
    host_dir: &std::path::Path,
    guest_tar_path: &str,
) -> anyhow::Result<()> {
    let temp = tempfile::NamedTempFile::with_suffix(".tar.gz")?;
    tar_directory_contents(host_dir, temp.path())?;

    let bytes = std::fs::read(temp.path())?;
    const CHUNK: usize = 2 * 1024;
    let client = backend.client()?;
    tracing::info!(
        "Pushing workspace {} into guest ({} bytes, {} chunks)",
        host_dir.display(),
        bytes.len(),
        bytes.len().div_ceil(CHUNK)
    );
    for (i, chunk) in bytes.chunks(CHUNK).enumerate() {
        match client.guest_request(GuestRequest::WriteChunk {
            path: guest_tar_path.to_string(),
            data_hex: to_hex(chunk),
            append: i > 0,
        })? {
            GuestResponse::Ok { .. } => {}
            GuestResponse::Error { message } => anyhow::bail!("WriteChunk failed: {}", message),
            other => anyhow::bail!("unexpected response to WriteChunk: {:?}", other),
        }
    }
    Ok(())
}

/// Read the guest-side exported workspace tarball back to the host and extract
/// it over the host workspace directory.
fn pull_workspace_from_guest(
    backend: &MacLinuxBackend,
    container_name: &str,
    guest_tar_path: &str,
    host_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let temp = tempfile::NamedTempFile::with_suffix(".tar.gz")?;
    read_guest_file_chunks(backend, guest_tar_path, temp.path())?;
    extract_tarball_contents(temp.path(), host_dir)?;
    tracing::info!(
        "Pulled workspace for container {} back to {}",
        container_name,
        host_dir.display()
    );
    Ok(())
}

fn read_guest_file_chunks(
    backend: &MacLinuxBackend,
    guest_path: &str,
    host_path: &std::path::Path,
) -> anyhow::Result<()> {
    let mut file = std::fs::File::create(host_path)?;
    let mut offset: u64 = 0;
    const CHUNK: usize = 2 * 1024;
    let client = backend.client()?;
    loop {
        match client.guest_request(GuestRequest::ReadChunk {
            path: guest_path.to_string(),
            offset,
            len: CHUNK,
        })? {
            GuestResponse::Chunk { data_hex, eof } => {
                let bytes = decode_hex(&data_hex)
                    .ok_or_else(|| anyhow::anyhow!("invalid hex chunk from guest"))?;
                use std::io::Write;
                file.write_all(&bytes)?;
                offset += bytes.len() as u64;
                if eof || bytes.is_empty() {
                    break;
                }
            }
            GuestResponse::Error { message } => anyhow::bail!("ReadChunk failed: {}", message),
            other => anyhow::bail!("unexpected response to ReadChunk: {:?}", other),
        }
    }
    Ok(())
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if s.len() % 2 != 0 {
        return None;
    }
    fn nib(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in s.chunks(2) {
        out.push((nib(pair[0])? << 4) | nib(pair[1])?);
    }
    Some(out)
}

fn tar_directory_contents(src_dir: &std::path::Path, dst_file: &std::path::Path) -> anyhow::Result<()> {
    let file = std::fs::File::create(dst_file)?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for entry in walkdir::WalkDir::new(src_dir).min_depth(1) {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(src_dir)?;
        builder.append_path_with_name(path, rel)?;
    }
    builder.finish()?;
    Ok(())
}

fn extract_tarball_contents(src_file: &std::path::Path, dst_dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst_dir)?;
    let file = std::fs::File::open(src_file)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(dst_dir)?;
    Ok(())
}

#[async_trait]
impl ExoBackend for MacLinuxBackend {
    async fn run(
        &self,
        config: ContainerConfig,
        _opts: BackendRunOptions,
    ) -> anyhow::Result<RunResult> {
        Self::validate_run_config(&config)?;
        // Provision the image rootfs into the guest so the run is isolated in
        // that rootfs rather than exec'd raw in the guest.
        self.ensure_image(&config.image).await?;

        let mut spec = Self::container_spec(&config);
        let workspace_host_path = config.workspace.clone();

        // The workspace is staged into and exported out of the container
        // workdir. With workdir `/` the export would archive the entire image
        // rootfs over the host workspace, so pin the workdir to a dedicated
        // directory whenever a workspace is set.
        if workspace_host_path.is_some() && (spec.workdir.is_empty() || spec.workdir == "/") {
            spec.workdir = "/app".to_string();
        }

        // Push the host workspace into the guest before the run. The tarball
        // contents are extracted into a guest staging area; the guest runtime
        // copies them into the container's workdir after mounting the overlay.
        let pushed_guest_tar = if let Some(ref host_ws) = workspace_host_path {
            if host_ws.exists() && host_ws.is_dir() {
                let guest_tar = format!("/tmp/exo-workspace-in-{}.tar.gz", spec.name);
                push_workspace_to_guest(self, host_ws, &guest_tar)?;
                let dest_dir = format!("/var/lib/exo-guest/workspaces/{}", spec.name);
                match self.client()?.guest_request(GuestRequest::PushWorkspace {
                    tar_path: guest_tar.clone(),
                    dest_dir: dest_dir.clone(),
                })? {
                    GuestResponse::Ok { .. } => {
                        // Tell the guest runtime where the staged workspace is so
                        // it can copy it into the container before exec.
                        spec.workspace = Some(dest_dir.clone());
                        Some((guest_tar, dest_dir))
                    }
                    GuestResponse::Error { message } => {
                        anyhow::bail!("PushWorkspace failed: {}", message)
                    }
                    other => anyhow::bail!("unexpected response to PushWorkspace: {:?}", other),
                }
            } else {
                tracing::warn!(
                    "workspace {} does not exist or is not a directory; skipping push",
                    host_ws.display()
                );
                None
            }
        } else {
            None
        };

        // Publish requested container ports on the host loopback through the
        // daemon's vsock tunnels. The guest connects 127.0.0.1:<container_port>
        // per accepted host connection, so the tunnels work for any process
        // listening inside the VM's shared network namespace.
        let mut published_host_ports: Vec<u16> = Vec::new();
        let tunnel_owner = Some(spec.name.clone());
        for port in &config.network.port_mappings {
            self.client()?
                .start_tunnel(port.host_port, port.container_port, tunnel_owner.clone())
                .map_err(|e| {
                    anyhow::anyhow!(
                        "publish {}:{} failed: {}",
                        port.host_port,
                        port.container_port,
                        e
                    )
                })?;
            published_host_ports.push(port.host_port);
        }

        let result: anyhow::Result<RunResult> = match self.client()?.guest_request(
            GuestRequest::RunContainer {
                spec,
                detach: _opts.detach,
                rm: _opts.rm,
            },
        )? {
            GuestResponse::RunResult {
                id,
                name,
                exit_code,
                stdout,
                stderr,
            } => {
                if !stderr.is_empty() {
                    tracing::warn!("guest stderr: {}", stderr);
                }
                Ok(RunResult {
                    id: Some(id),
                    name,
                    message: stdout,
                    exit_code,
                })
            }
            GuestResponse::Error { message } => Err(anyhow::anyhow!("{}", message)),
            other => Err(anyhow::anyhow!(
                "unexpected guest response to RunContainer: {:?}",
                other
            )),
        };

        // Foreground runs are done: tear the tunnels down. Detached containers
        // keep serving, so their tunnels stay up until the container is stopped
        // or removed through the daemon (matched by container name) or the VM
        // stops.
        if !_opts.detach {
            for host_port in published_host_ports {
                if let Err(e) = self.client()?.stop_tunnel(host_port) {
                    tracing::warn!("failed to stop tunnel on :{}: {}", host_port, e);
                }
            }
        }

        let result = result?;

        // Pull the workspace back after the run so host-side artifacts are
        // persisted. The guest exports the modified overlay upper layer to a
        // known /tmp path before removing the container. Detached containers
        // are still running, so there is nothing to export yet — skip the
        // pull (and its guaranteed "no such file" warning) for them.
        if !_opts.detach && pushed_guest_tar.is_some() {
            if let Some(ref host_ws) = workspace_host_path {
                let guest_out_tar = format!("/tmp/exo-workspace-out-{}.tar.gz", result.name);
                if let Err(e) =
                    pull_workspace_from_guest(self, &result.name, &guest_out_tar, host_ws)
                {
                    tracing::warn!("failed to pull workspace back from guest: {}", e);
                }
            }
        }

        Ok(result)
    }

    async fn list(&self, _opts: ListOptions) -> anyhow::Result<Vec<ContainerMetadata>> {
        match self
            .client()?
            .guest_request(GuestRequest::ListContainers { all: _opts.all })?
        {
            GuestResponse::ContainerList { containers } => Ok(containers
                .into_iter()
                .map(|summary| {
                    let mut metadata = ContainerMetadata::new(
                        summary.name.clone(),
                        ContainerConfig {
                            name: summary.name.clone(),
                            image: summary.image.clone(),
                            ..Default::default()
                        },
                    );
                    metadata.id = summary.id;
                    metadata.status = summary.status;
                    metadata.pid = summary.pid;
                    metadata.ports = summary.ports;
                    metadata
                })
                .collect()),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to ListContainers: {:?}", other),
        }
    }

    async fn start(&self, _id: &str, _opts: StartOptions) -> anyhow::Result<()> {
        // Re-establish port tunnels before telling the guest to start. The
        // tunnels are owned by the host daemon and are torn down when the
        // container stops; starting a container must recreate them.
        let container = match self.client()?.guest_request(GuestRequest::GetContainer {
            id_or_name: _id.to_string(),
        })? {
            GuestResponse::ContainerInfo {
                id,
                name,
                ports,
                ..
            } => (id, name, ports),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected response to GetContainer: {:?}", other),
        };

        for port_str in &container.2 {
            let Some((host_s, guest_s)) = port_str.split_once(':') else {
                tracing::warn!("ignoring malformed port mapping '{}'", port_str);
                continue;
            };
            let Ok(host_port) = host_s.parse::<u16>() else {
                tracing::warn!("ignoring malformed host port '{}'", host_s);
                continue;
            };
            let Ok(guest_port) = guest_s.parse::<u16>() else {
                tracing::warn!("ignoring malformed guest port '{}'", guest_s);
                continue;
            };
            if let Err(e) = self
                .client()?
                .start_tunnel(host_port, guest_port, Some(container.1.clone()))
            {
                // If the tunnel is already bound (e.g. container was never fully
                // stopped), treat it as success rather than fail the start.
                if !e.to_string().contains("already bound") {
                    anyhow::bail!(
                        "failed to rebind tunnel {}:{} for container {}: {}",
                        host_port,
                        guest_port,
                        container.0,
                        e
                    );
                }
                tracing::info!(
                    "tunnel {}:{} already bound for container {}",
                    host_port,
                    guest_port,
                    container.0
                );
            }
        }

        match self.client()?.guest_request(GuestRequest::StartContainer {
            id: container.0,
            attach: _opts.attach,
        })? {
            GuestResponse::Ok { .. } => Ok(()),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to StartContainer: {:?}", other),
        }
    }

    async fn stop(&self, _id: &str, _opts: StopOptions) -> anyhow::Result<()> {
        match self.client()?.guest_request(GuestRequest::StopContainer {
            id: _id.to_string(),
            force: _opts.force,
            timeout_secs: _opts.timeout_secs,
        })? {
            GuestResponse::Ok { .. } => Ok(()),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to StopContainer: {:?}", other),
        }
    }

    async fn remove(&self, _id: &str, _opts: RemoveOptions) -> anyhow::Result<()> {
        match self
            .client()?
            .guest_request(GuestRequest::RemoveContainer {
                id: _id.to_string(),
                force: _opts.force,
            })? {
            GuestResponse::Ok { .. } => Ok(()),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to RemoveContainer: {:?}", other),
        }
    }

    async fn logs(&self, _id: &str, _opts: BackendLogOptions) -> anyhow::Result<LogStream> {
        if _opts.follow {
            anyhow::bail!("follow-mode logs are not implemented for the EXO macOS Linux VM yet");
        }
        match self.client()?.guest_request(GuestRequest::Logs {
            id: _id.to_string(),
            follow: _opts.follow,
            tail: _opts.tail,
            timestamps: _opts.timestamps,
        })? {
            GuestResponse::Logs { content } => Ok(LogStream { content }),
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to Logs: {:?}", other),
        }
    }

    async fn exec(
        &self,
        _id: &str,
        _command: Vec<String>,
        _opts: ExecOptions,
    ) -> anyhow::Result<i32> {
        if _opts.interactive || _opts.tty {
            anyhow::bail!("interactive/TTY exec is not implemented for the EXO macOS Linux VM yet");
        }
        match self.client()?.guest_request(GuestRequest::Exec {
            id: _id.to_string(),
            command: _command,
            user: _opts.user,
            interactive: _opts.interactive,
            tty: _opts.tty,
        })? {
            GuestResponse::ExecResult {
                exit_code,
                stdout,
                stderr,
            } => {
                if !stdout.is_empty() {
                    print!("{}", stdout);
                }
                if !stderr.is_empty() {
                    eprint!("{}", stderr);
                }
                Ok(exit_code)
            }
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to Exec: {:?}", other),
        }
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::macos_linux_microvm()
    }
}

fn parse_memory_mb(size: &str) -> Option<u64> {
    let size = size.trim().to_lowercase();
    let (num, multiplier) = if let Some(num) = size.strip_suffix('g') {
        (num, 1024)
    } else if let Some(num) = size.strip_suffix('m') {
        (num, 1)
    } else {
        return size.parse::<u64>().ok().map(|bytes| bytes / 1024 / 1024);
    };
    num.parse::<u64>().ok().map(|n| n * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exo_runtime::config::PortMapping;
    use exo_runtime::{MountConfig, NetworkConfig, ResourceConfig};
    use std::path::PathBuf;

    #[test]
    fn converts_container_config_to_guest_spec() {
        let config = ContainerConfig {
            name: "app".to_string(),
            image: "python:3.12".to_string(),
            command: vec!["python".to_string(), "app.py".to_string()],
            workdir: PathBuf::from("/app"),
            env: [("A".to_string(), "B".to_string())].into_iter().collect(),
            mounts: vec![MountConfig {
                mount_type: "bind".to_string(),
                source: "/host/app".to_string(),
                target: "/app".to_string(),
                readonly: false,
                size: None,
                propagation: "rprivate".to_string(),
            }],
            network: NetworkConfig {
                port_mappings: vec![PortMapping {
                    host_port: 8080,
                    container_port: 8000,
                    protocol: "tcp".to_string(),
                    host_ip: "127.0.0.1".to_string(),
                }],
                ..Default::default()
            },
            resources: ResourceConfig {
                memory: Some("512m".to_string()),
                cpu: Some("1".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let spec = MacLinuxBackend::container_spec(&config);
        assert_eq!(spec.name, "app");
        assert_eq!(spec.image, "python:3.12");
        assert_eq!(spec.workdir, "/app");
        assert_eq!(spec.env, vec!["A=B".to_string()]);
        assert_eq!(spec.mounts[0].target, "/app");
        assert_eq!(spec.network.mode, "bridge");
        assert_eq!(spec.network.ports[0].host_port, 8080);
        assert_eq!(spec.memory_mb, Some(512));
    }

    #[test]
    fn capabilities_are_linux_container_oriented() {
        let backend = MacLinuxBackend::new(crate::VmConfig::default());
        let caps = backend.capabilities();
        assert!(caps.linux_containers);
        assert!(caps.overlayfs);
        assert!(caps.daemon);
        assert!(!caps.namespaces);
        assert!(!caps.cgroups);
        assert!(!caps.seccomp);
        assert!(!caps.rootless);
        assert!(!caps.native_processes);
    }

    #[test]
    fn rejects_unenforced_network_and_host_mounts() {
        let mut config = ContainerConfig::default();
        // The default bridge mode maps to guest NAT: allowed.
        assert!(MacLinuxBackend::validate_run_config(&config).is_ok());

        // TCP host port publishing goes through the vsock tunnel: allowed.
        config
            .network
            .port_mappings
            .push(PortMapping {
                host_ip: "127.0.0.1".to_string(),
                host_port: 8080,
                container_port: 80,
                protocol: "tcp".to_string(),
            });
        assert!(MacLinuxBackend::validate_run_config(&config).is_ok());

        // UDP publishing is not supported by the tunnel.
        config.network.port_mappings[0].protocol = "udp".to_string();
        assert!(MacLinuxBackend::validate_run_config(&config).is_err());
        config.network.port_mappings.clear();

        // An unknown mode is still rejected.
        config.network.mode = "host".to_string();
        assert!(MacLinuxBackend::validate_run_config(&config).is_err());

        config.network.mode = "none".to_string();
        assert!(MacLinuxBackend::validate_run_config(&config).is_ok());

        config.mounts.push(exo_runtime::MountConfig {
            mount_type: "bind".to_string(),
            source: "/host/project".to_string(),
            target: "/project".to_string(),
            readonly: true,
            size: None,
            propagation: "rprivate".to_string(),
        });
        assert!(MacLinuxBackend::validate_run_config(&config).is_err());
    }
}
