//! Minimal guest agent for the Exo Linux microVM.
//!
//! This binary runs as PID 1 inside an initramfs on an Apple
//! Virtualization.framework VM. It reads JSON requests from stdin and writes
//! newline-terminated JSON responses to stdout over a dedicated virtio console
//! serial port.

mod runtime;

use runtime::{ContainerSpec, ContainerSummary, GuestRuntime};
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "method")]
enum GuestRequest {
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
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status")]
enum GuestResponse {
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
    Error {
        message: String,
    },
}

fn handle_request(line: &str, started: Instant) -> GuestResponse {
    let runtime = match GuestRuntime::new_default() {
        Ok(runtime) => runtime,
        Err(e) => {
            return GuestResponse::Error {
                message: format!("runtime init failed: {}", e),
            };
        }
    };
    handle_request_with_runtime(line, started, &runtime)
}

fn handle_request_with_runtime(
    line: &str,
    started: Instant,
    runtime: &GuestRuntime,
) -> GuestResponse {
    let req = match serde_json::from_str::<GuestRequest>(line) {
        Ok(r) => r,
        Err(e) => {
            return GuestResponse::Error {
                message: format!("invalid request: {}", e),
            };
        }
    };
    match req {
        GuestRequest::Ping => GuestResponse::Pong,
        GuestRequest::Status => GuestResponse::Status {
            uptime_secs: started.elapsed().as_secs(),
        },
        GuestRequest::Stop => {
            trigger_power_off();
            GuestResponse::Ok {
                message: "power off initiated".to_string(),
            }
        }
        GuestRequest::RunContainer { spec, detach, rm } => {
            match runtime.run_container(spec, detach, rm) {
                Ok(out) => GuestResponse::RunResult {
                    id: out.id,
                    name: out.name,
                    exit_code: out.exit_code,
                    stdout: out.stdout,
                    stderr: out.stderr,
                },
                Err(e) => GuestResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        GuestRequest::ListContainers { all } => match runtime.list_containers(all) {
            Ok(containers) => GuestResponse::ContainerList { containers },
            Err(e) => GuestResponse::Error {
                message: e.to_string(),
            },
        },
        GuestRequest::StartContainer { id, .. } => GuestResponse::Error {
            message: format!("container start is not implemented yet for {}", id),
        },
        GuestRequest::StopContainer {
            id,
            force,
            timeout_secs: _,
        } => match runtime.stop_container(&id, force) {
            Ok(()) => GuestResponse::Ok {
                message: format!("container {} stopped", id),
            },
            Err(e) => GuestResponse::Error {
                message: e.to_string(),
            },
        },
        GuestRequest::RemoveContainer { id, force } => match runtime.remove_container(&id, force) {
            Ok(()) => GuestResponse::Ok {
                message: format!("container {} removed", id),
            },
            Err(e) => GuestResponse::Error {
                message: e.to_string(),
            },
        },
        GuestRequest::Logs { id, tail, .. } => match runtime.logs(&id, tail) {
            Ok(content) => GuestResponse::Logs { content },
            Err(e) => GuestResponse::Error {
                message: e.to_string(),
            },
        },
        GuestRequest::Exec { id, command, .. } => match runtime.exec(&id, command) {
            Ok((exit_code, stdout, stderr)) => GuestResponse::ExecResult {
                exit_code,
                stdout,
                stderr,
            },
            Err(e) => GuestResponse::Error {
                message: e.to_string(),
            },
        },
        GuestRequest::ImportImage { image, tar_path } => {
            match runtime.import_image_from_tar(&image, std::path::Path::new(&tar_path)) {
                Ok(rootfs) => GuestResponse::ImageImported {
                    image,
                    rootfs_path: rootfs.display().to_string(),
                },
                Err(e) => GuestResponse::Error {
                    message: e.to_string(),
                },
            }
        }
    }
}

fn trigger_power_off() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::sync();
        // LINUX_REBOOT_CMD_POWER_OFF
        let _ = libc::syscall(
            libc::SYS_reboot,
            0xfee1deadu32 as i32,
            0x28121969u32 as i32,
            0x4321fedcu32 as i32,
            0,
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        // No-op on non-Linux hosts; this binary is only meant to run inside the VM.
    }
}

fn main() {
    eprintln!("exo-vm-guest-init started");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = stdout.lock();
    let started = Instant::now();

    eprintln!("Listening on serial console for JSON RPC");

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                eprintln!("Host closed serial connection");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let resp = handle_request(trimmed, started);
                let json = match serde_json::to_string(&resp) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("JSON encode error: {}", e);
                        continue;
                    }
                };
                if let Err(e) = writeln!(stdout, "{}", json) {
                    eprintln!("Write failed: {}", e);
                }
                if let Err(e) = stdout.flush() {
                    eprintln!("Flush failed: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Read failed: {}", e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::NetworkSpec;

    fn spec(command: Vec<&str>) -> ContainerSpec {
        ContainerSpec {
            name: "test".to_string(),
            image: "alpine:latest".to_string(),
            command: command.into_iter().map(String::from).collect(),
            workdir: "/".to_string(),
            env: vec!["EXO_TEST_VALUE=ok".to_string()],
            mounts: vec![],
            network: NetworkSpec {
                mode: "bridge".to_string(),
                ports: vec![],
                dns: vec![],
            },
            gpu: false,
            memory_mb: None,
            cpu: None,
        }
    }

    #[test]
    fn ping_roundtrips() {
        let rt = GuestRuntime::new(tempfile::tempdir().unwrap().path()).unwrap();
        let resp = handle_request_with_runtime(r#"{"method":"Ping"}"#, Instant::now(), &rt);
        assert!(matches!(resp, GuestResponse::Pong));
    }

    #[test]
    fn run_container_executes_command() {
        let req = GuestRequest::RunContainer {
            spec: spec(vec!["sh", "-c", "printf %s \"$EXO_TEST_VALUE\""]),
            detach: false,
            rm: true,
        };
        let line = serde_json::to_string(&req).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let rt = GuestRuntime::new(temp.path()).unwrap();
        let resp = handle_request_with_runtime(&line, Instant::now(), &rt);
        match resp {
            GuestResponse::RunResult {
                exit_code, stdout, ..
            } => {
                assert_eq!(exit_code, Some(0));
                assert_eq!(stdout, "ok");
            }
            other => panic!("unexpected response: {:?}", other),
        }
    }

    #[test]
    fn run_container_reports_nonzero_exit() {
        let req = GuestRequest::RunContainer {
            spec: spec(vec!["sh", "-c", "exit 7"]),
            detach: false,
            rm: true,
        };
        let line = serde_json::to_string(&req).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let rt = GuestRuntime::new(temp.path()).unwrap();
        let resp = handle_request_with_runtime(&line, Instant::now(), &rt);
        match resp {
            GuestResponse::RunResult { exit_code, .. } => {
                assert_eq!(exit_code, Some(7));
            }
            other => panic!("unexpected response: {:?}", other),
        }
    }

    #[test]
    fn list_containers_returns_empty_list_for_now() {
        let rt = GuestRuntime::new(tempfile::tempdir().unwrap().path()).unwrap();
        let resp = handle_request_with_runtime(
            r#"{"method":"ListContainers","all":true}"#,
            Instant::now(),
            &rt,
        );
        match resp {
            GuestResponse::ContainerList { containers } => assert!(containers.is_empty()),
            other => panic!("unexpected response: {:?}", other),
        }
    }
}
