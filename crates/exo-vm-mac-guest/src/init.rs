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
    /// Append a hex-encoded byte chunk to a guest file. Used to stream an image
    /// tarball from the host into the guest before ImportImage, since the guest
    /// has no network downloader.
    WriteChunk {
        path: String,
        data_hex: String,
        append: bool,
    },
    /// Report whether an image rootfs is already present in the guest store.
    ImageExists {
        image: String,
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
        GuestRequest::WriteChunk {
            path,
            data_hex,
            append,
        } => match write_chunk(&path, &data_hex, append) {
            Ok(written) => GuestResponse::Ok {
                message: format!("wrote {} bytes to {}", written, path),
            },
            Err(e) => GuestResponse::Error {
                message: e.to_string(),
            },
        },
        GuestRequest::ImageExists { image } => {
            // Always Ok so "absent" isn't treated as a transport error; the host
            // distinguishes on the message.
            let present = runtime.image_rootfs_dir(&image).exists();
            GuestResponse::Ok {
                message: if present { "present" } else { "absent" }.to_string(),
            }
        }
    }
}

/// Decode a hex string and append (or truncate-write) it to a guest file.
fn write_chunk(path: &str, data_hex: &str, append: bool) -> std::io::Result<usize> {
    use std::io::Write;
    let bytes = decode_hex(data_hex)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad hex chunk"))?;
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)?;
    file.write_all(&bytes)?;
    Ok(bytes.len())
}

/// Minimal hex decoder (avoids pulling in a crate for the guest).
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

    // RPC travels over the dedicated virtio-console port (hvc1). hvc0 is the
    // console/log port that the kernel wires to this process's stdio, so reading
    // RPC from stdin would listen on the wrong channel (the host drives hvc1) and
    // every request would time out. Open /dev/hvc1 explicitly for request/response;
    // logging stays on stderr (hvc0 → console log). Fall back to stdin/stdout if
    // the port is unavailable, so a misconfigured guest still limps rather than
    // silently hangs.
    let started = Instant::now();

    // Try the dedicated RPC port first; on any failure, fall back to stdio so a
    // misconfigured guest limps instead of silently hanging.
    let mut reader: Box<dyn BufRead>;
    let mut writer: Box<dyn Write>;
    match open_rpc_port() {
        Some((r, w)) => {
            eprintln!("Listening on /dev/hvc1 for JSON RPC");
            reader = r;
            writer = w;
        }
        None => {
            eprintln!("Falling back to stdin/stdout for JSON RPC");
            reader = Box::new(BufReader::new(std::io::stdin()));
            writer = Box::new(std::io::stdout());
        }
    }

    run_rpc_loop(&mut *reader, &mut *writer, started);
}

/// Open the dedicated virtio-console RPC port (`/dev/hvc1`) for read and write.
/// Returns a buffered reader and a writer over independent handles to the same
/// port, or `None` if the port cannot be opened/cloned.
fn open_rpc_port() -> Option<(Box<dyn BufRead>, Box<dyn Write>)> {
    let port = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/hvc1")
        .map_err(|e| eprintln!("Could not open /dev/hvc1: {e}"))
        .ok()?;

    // /dev/hvc1 is a virtio-console TTY that defaults to canonical mode with echo
    // enabled. Echo would reflect each host request back over the same port, and
    // the host would parse its own `{"method":...}` line as our response ("missing
    // field `status`"). Put the port in raw mode so only agent-written bytes flow
    // back to the host.
    make_raw(&port);

    let read_side = port
        .try_clone()
        .map_err(|e| eprintln!("Could not clone /dev/hvc1: {e}"))
        .ok()?;
    Some((Box::new(BufReader::new(read_side)), Box::new(port)))
}

/// Put a TTY file descriptor into raw mode (no echo, no canonical line editing,
/// no output translation) so a line-delimited binary-safe protocol works over it.
/// Best-effort: a non-TTY or a failed ioctl is ignored.
fn make_raw(port: &std::fs::File) {
    use std::os::unix::io::AsRawFd;
    let fd = port.as_raw_fd();
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            return;
        }
        libc::cfmakeraw(&mut termios);
        let _ = libc::tcsetattr(fd, libc::TCSANOW, &mut termios);
    }
}

/// Serve JSON-RPC requests line-by-line until the transport closes for good.
fn run_rpc_loop(reader: &mut dyn BufRead, stdout: &mut dyn Write, started: Instant) {
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
