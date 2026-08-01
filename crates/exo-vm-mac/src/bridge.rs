use serde::{Deserialize, Serialize};

/// Mount exposed to a Linux container in the guest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MountSpec {
    pub source: String,
    pub target: String,
    pub readonly: bool,
}

/// Port mapping requested by the host CLI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortSpec {
    pub host_ip: String,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

/// Guest-side network intent for a container.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSpec {
    pub mode: String,
    pub ports: Vec<PortSpec>,
    pub dns: Vec<String>,
}

/// Container run specification sent to the guest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub workdir: String,
    /// Host workspace directory streamed into the container before the run and
    /// pulled back after. The value is the guest path where the workspace is
    /// extracted (typically the container workdir).
    pub workspace: Option<String>,
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
#[serde(tag = "method")]
pub enum GuestRequest {
    Ping,
    Status,
    Stop,
    RunContainer {
        spec: ContainerSpec,
        detach: bool,
        rm: bool,
    },
    ListContainers {
        all: bool,
    },
    StartContainer {
        id: String,
        attach: bool,
    },
    StopContainer {
        id: String,
        force: bool,
        timeout_secs: u64,
    },
    RemoveContainer {
        id: String,
        force: bool,
    },
    Logs {
        id: String,
        follow: bool,
        tail: usize,
        timestamps: bool,
    },
    Exec {
        id: String,
        command: Vec<String>,
        user: Option<String>,
        interactive: bool,
        tty: bool,
    },
    ImportImage {
        image: String,
        tar_path: String,
    },
    /// Append a hex-encoded byte chunk to a guest file (image tarball transfer).
    WriteChunk {
        path: String,
        data_hex: String,
        append: bool,
    },
    /// Ask whether an image rootfs is already present in the guest store.
    ImageExists {
        image: String,
    },
    /// Extract a host-streamed tarball into a guest directory before a container
    /// run. The tarball must already exist at `tar_path` (written via WriteChunk).
    PushWorkspace {
        tar_path: String,
        dest_dir: String,
    },
    /// Create a gzipped tarball of a guest directory so the host can pull it back.
    ExportWorkspace {
        source_dir: String,
        tar_path: String,
    },
    /// Read a byte range from a guest file and return it as a hex-encoded chunk.
    ReadChunk {
        path: String,
        offset: u64,
        len: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum GuestResponse {
    Pong,
    Status {
        uptime_secs: u64,
    },
    Ok {
        message: String,
    },
    RunResult {
        id: String,
        name: String,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    ContainerList {
        containers: Vec<ContainerSummary>,
    },
    Logs {
        content: String,
    },
    ExecResult {
        exit_code: i32,
        stdout: String,
        stderr: String,
    },
    ImageImported {
        image: String,
        rootfs_path: String,
    },
    /// Hex-encoded byte chunk returned by ReadChunk.
    Chunk {
        data_hex: String,
        eof: bool,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_container_request_roundtrips() {
        let req = GuestRequest::RunContainer {
            spec: ContainerSpec {
                name: "web".to_string(),
                image: "alpine:latest".to_string(),
                command: vec!["echo".to_string(), "hello".to_string()],
                workdir: "/app".to_string(),
                workspace: None,
                env: vec!["A=B".to_string()],
                mounts: vec![MountSpec {
                    source: "/host".to_string(),
                    target: "/app".to_string(),
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
                    dns: vec!["1.1.1.1".to_string()],
                },
                gpu: false,
                memory_mb: Some(256),
                cpu: Some("1".to_string()),
            },
            detach: false,
            rm: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("RunContainer"));
        let decoded: GuestRequest = serde_json::from_str(&json).unwrap();
        match decoded {
            GuestRequest::RunContainer { spec, detach, rm } => {
                assert_eq!(spec.image, "alpine:latest");
                assert!(!detach);
                assert!(rm);
            }
            _ => panic!("unexpected request"),
        }
    }

    #[test]
    fn response_roundtrips() {
        let resp = GuestResponse::ContainerList {
            containers: vec![ContainerSummary {
                id: "abc".to_string(),
                name: "test".to_string(),
                image: "alpine".to_string(),
                status: "running".to_string(),
                pid: Some(42),
                ports: vec!["8080:80".to_string()],
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: GuestResponse = serde_json::from_str(&json).unwrap();
        match decoded {
            GuestResponse::ContainerList { containers } => {
                assert_eq!(containers[0].name, "test");
            }
            _ => panic!("unexpected response"),
        }
    }
}
