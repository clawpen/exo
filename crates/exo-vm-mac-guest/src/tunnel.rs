//! TCP tunneling into the guest over virtio-vsock.
//!
//! The host cannot reach the VM's NAT address, so host-published container
//! ports are tunneled: the host daemon listens on 127.0.0.1:<host_port> and,
//! per accepted connection, opens a vsock connection to this listener and
//! sends a 2-byte big-endian target port. We dial 127.0.0.1:<target_port>
//! inside the guest (containers share the guest network namespace via chroot)
//! and pump bytes in both directions.
//!
//! Each vsock connection is independent, so concurrent host connections never
//! interleave — unlike the single shared serial RPC channel.

#![cfg(target_os = "linux")]

use std::io::Read;
use std::os::unix::io::FromRawFd;
use std::time::Duration;

const AF_VSOCK: libc::sa_family_t = 40; // AF_VSOCK (not exposed by musl libc crate)
const VMADDR_CID_ANY: u32 = u32::MAX; // listen on all contexts

/// vsock port the tunnel listener binds. The host daemon connects here and
/// prefixes each connection with the real target port.
pub const TUNNEL_VSOCK_PORT: u32 = 1025;

#[repr(C)]
struct SockaddrVm {
    svm_family: u16,
    svm_reserved1: u16,
    svm_port: u32,
    svm_cid: u32,
    svm_zero: [u8; 4],
}

/// Accept vsock connections forever, one thread per connection. Returns early
/// (logging to the console) when vsock support is unavailable so the guest
/// agent keeps serving serial RPC.
pub fn serve_vsock_tunnels() {
    unsafe {
        let listener = libc::socket(AF_VSOCK as i32, libc::SOCK_STREAM, 0);
        if listener < 0 {
            eprintln!(
                "vsock tunnels unavailable (socket: {}); port publishing disabled",
                std::io::Error::last_os_error()
            );
            return;
        }
        let addr = SockaddrVm {
            svm_family: AF_VSOCK as u16,
            svm_reserved1: 0,
            svm_port: TUNNEL_VSOCK_PORT,
            svm_cid: VMADDR_CID_ANY,
            svm_zero: [0; 4],
        };
        if libc::bind(
            listener,
            &addr as *const SockaddrVm as *const libc::sockaddr,
            std::mem::size_of::<SockaddrVm>() as u32,
        ) != 0
        {
            eprintln!(
                "vsock tunnels unavailable (bind: {}); port publishing disabled",
                std::io::Error::last_os_error()
            );
            libc::close(listener);
            return;
        }
        if libc::listen(listener, 128) != 0 {
            eprintln!(
                "vsock tunnels unavailable (listen: {}); port publishing disabled",
                std::io::Error::last_os_error()
            );
            libc::close(listener);
            return;
        }
        eprintln!("Listening on vsock port {} for port tunnels", TUNNEL_VSOCK_PORT);
        loop {
            let conn = libc::accept(listener, std::ptr::null_mut(), std::ptr::null_mut());
            if conn < 0 {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            std::thread::spawn(move || handle_connection(conn));
        }
    }
}

fn handle_connection(fd: i32) {
    let mut vsock = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut header = [0u8; 2];
    if vsock.read_exact(&mut header).is_err() {
        return;
    }
    let target_port = u16::from_be_bytes(header);
    let target = match std::net::TcpStream::connect(("127.0.0.1", target_port)) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("tunnel: dial 127.0.0.1:{} failed: {}", target_port, e);
            return;
        }
    };

    let mut vsock_writer = match vsock.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    };
    let mut target_writer = match target.try_clone() {
        Ok(clone) => clone,
        Err(_) => return,
    };

    let uplink = std::thread::spawn(move || {
        let _ = std::io::copy(&mut vsock, &mut target_writer);
    });
    let mut target = target;
    let _ = std::io::copy(&mut target, &mut vsock_writer);
    let _ = uplink.join();
}
