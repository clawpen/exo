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

    fn client(&self) -> anyhow::Result<crate::VmDaemonClient> {
        let client = crate::VmDaemonClient::new()?;
        if client.is_running() {
            Ok(client)
        } else {
            Err(self.not_ready())
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
        let Some(url) = Self::image_rootfs_url(image) else {
            return Ok(());
        };
        if let Ok(GuestResponse::Ok { message }) = self.client()?.guest_request(GuestRequest::ImageExists {
            image: image.to_string(),
        }) {
            if message == "present" {
                return Ok(());
            }
        }

        let safe: String = image
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let cache = crate::paths::exo_vm_dir()?.join("images");
        std::fs::create_dir_all(&cache)?;
        let host_tar = cache.join(format!("{}.tar.gz", safe));
        crate::image::download_file_if_missing(url, &host_tar).await?;

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

#[async_trait]
impl ExoBackend for MacLinuxBackend {
    async fn run(
        &self,
        config: ContainerConfig,
        _opts: BackendRunOptions,
    ) -> anyhow::Result<RunResult> {
        // Provision the image rootfs into the guest so the run is isolated in
        // that rootfs rather than exec'd raw in the guest.
        self.ensure_image(&config.image).await?;

        let spec = Self::container_spec(&config);
        match self.client()?.guest_request(GuestRequest::RunContainer {
            spec,
            detach: _opts.detach,
            rm: _opts.rm,
        })? {
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
            GuestResponse::Error { message } => anyhow::bail!("{}", message),
            other => anyhow::bail!("unexpected guest response to RunContainer: {:?}", other),
        }
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
        match self.client()?.guest_request(GuestRequest::StartContainer {
            id: _id.to_string(),
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
        assert!(caps.namespaces);
        assert!(caps.cgroups);
        assert!(!caps.native_processes);
    }
}
