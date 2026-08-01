use crate::bridge::{GuestRequest, GuestResponse};
use crate::ffi::{exo_vm_rpc_fds, ExoVmHandle};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::FromRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Send a JSON request to the guest agent over the dedicated RPC serial socket
/// and parse the newline-terminated JSON response.
pub fn request(
    handle: *mut ExoVmHandle,
    _port: u32,
    req: GuestRequest,
    timeout_ms: u32,
) -> anyhow::Result<GuestResponse> {
    if handle.is_null() {
        anyhow::bail!("VM handle is null");
    }

    let mut read_fd: std::os::raw::c_int = -1;
    let mut write_fd: std::os::raw::c_int = -1;
    let ret = unsafe { exo_vm_rpc_fds(handle, &mut read_fd, &mut write_fd) };
    if ret != 0 || read_fd < 0 || write_fd < 0 {
        anyhow::bail!("RPC serial port not available");
    }

    // The socketpair FD is bidirectional and owned by the VM handle. Duplicate
    // it so the temporary UnixStream can be dropped (closing only the dup)
    // without closing the VM's underlying RPC fd, which would break subsequent
    // requests.
    let timeout = Duration::from_millis(timeout_ms as u64);
    let dup_fd = unsafe { libc::dup(read_fd) };
    if dup_fd < 0 {
        return Err(std::io::Error::last_os_error())
            .map_err(|e| anyhow::anyhow!("failed to dup RPC fd: {}", e));
    }

    // Virtualization.framework hands back the host socketpair end in
    // non-blocking mode. std's UnixStream read/write assume blocking semantics,
    // so without clearing O_NONBLOCK the very first read returns EAGAIN
    // ("Resource temporarily unavailable", os error 35) and the guest looks
    // unreachable even though it is answering. Clear it on the dup we own.
    unsafe {
        let flags = libc::fcntl(dup_fd, libc::F_GETFL, 0);
        if flags < 0 || libc::fcntl(dup_fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) < 0 {
            let err = std::io::Error::last_os_error();
            libc::close(dup_fd);
            return Err(anyhow::anyhow!("failed to set RPC fd blocking: {}", err));
        }
    }

    let mut stream = unsafe { UnixStream::from_raw_fd(dup_fd) };
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let json_in = serde_json::to_string(&req)?;
    writeln!(stream, "{}", json_in)?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let line = line.trim();
    if line.is_empty() {
        anyhow::bail!("empty guest response");
    }
    let response: GuestResponse = serde_json::from_str(line)?;
    Ok(response)
}
