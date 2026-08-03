//! Host side of the vsock port tunnel.
//!
//! Publishes a guest TCP port on the host loopback: a listener thread accepts
//! on 127.0.0.1:<host_port>, each accepted connection opens a fresh vsock
//! connection to the guest's tunnel listener and sends the 2-byte target port,
//! then bytes pump in both directions. The guest side dials 127.0.0.1:<port>
//! inside the VM, where chrooted containers share the network namespace.

use crate::ffi::{exo_vm_vsock_connect, exo_vm_vsock_disconnect, ExoVmHandle};
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::io::FromRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::warn;

/// vsock port the guest tunnel listener binds; mirrored in the guest agent.
pub const TUNNEL_VSOCK_PORT: u32 = 1025;

/// Raw VM handle made sendable for tunnel threads. Virtualization.framework
/// serializes VM work on its own dispatch queue, and the existing RPC path
/// already calls into the handle from client threads.
#[derive(Debug, Clone, Copy)]
pub struct SendableHandle(pub *mut ExoVmHandle);
unsafe impl Send for SendableHandle {}
unsafe impl Sync for SendableHandle {}

/// A running host port tunnel. Dropping it stops the listener thread.
pub struct HostTunnel {
    host_port: u16,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HostTunnel {
    pub fn host_port(&self) -> u16 {
        self.host_port
    }

    pub fn start(handle: SendableHandle, host_port: u16, guest_port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", host_port)).map_err(|e| {
            anyhow::anyhow!("bind 127.0.0.1:{} for tunnel: {}", host_port, e)
        })?;
        listener.set_nonblocking(true)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let handle = handle;
                        std::thread::spawn(move || pump_connection(handle, guest_port, stream));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        warn!("tunnel :{} accept failed: {}", host_port, e);
                        break;
                    }
                }
            }
        });
        Ok(Self {
            host_port,
            shutdown,
            thread: Some(thread),
        })
    }
}

impl Drop for HostTunnel {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn pump_connection(handle: SendableHandle, guest_port: u16, stream: std::net::TcpStream) {
    // Defensive: accepted sockets should already be blocking, but never let a
    // nonblocking stream into io::copy.
    let _ = stream.set_nonblocking(false);

    let fd = unsafe { exo_vm_vsock_connect(handle.0, TUNNEL_VSOCK_PORT, 10_000) };
    if fd < 0 {
        warn!("tunnel: vsock connect to guest port {} failed", guest_port);
        return;
    }
    // The returned fd is owned by the VZVirtioSocketConnection and closed when
    // that object is released. Dup it so Rust owns an independent descriptor,
    // then release the VZ connection immediately — closing its fd leaves the
    // socket alive because our dup still references it.
    let owned = unsafe { libc::dup(fd) };
    unsafe { exo_vm_vsock_disconnect(handle.0, fd) };
    if owned < 0 {
        warn!("tunnel: dup of vsock fd failed: {}", std::io::Error::last_os_error());
        return;
    }
    let mut vsock_out = unsafe { std::fs::File::from_raw_fd(owned) };
    if let Err(e) = vsock_out.write_all(&guest_port.to_be_bytes()) {
        warn!("tunnel: write target port failed: {}", e);
        return;
    }

    let mut vsock_in = match vsock_out.try_clone() {
        Ok(f) => f,
        Err(e) => {
            warn!("tunnel: clone vsock fd failed: {}", e);
            return;
        }
    };
    let mut host_in = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            warn!("tunnel: clone host stream failed: {}", e);
            return;
        }
    };
    let mut host_out = stream;

    // host -> guest; half-close the vsock on EOF so the guest tunnel handler
    // sees the end of the stream and closes its side.
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut host_in, &mut vsock_out);
        unsafe { libc::shutdown(owned, libc::SHUT_WR) };
    });
    // guest -> host
    let _ = std::io::copy(&mut vsock_in, &mut host_out);
    // The guest closed its side; unblock the uplink thread if it is still
    // waiting for host bytes so this thread can exit.
    let _ = host_out.shutdown(std::net::Shutdown::Both);
    let _ = up.join();
}
