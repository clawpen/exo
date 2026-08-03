use crate::agent_client;
use crate::bridge::{GuestRequest, GuestResponse};
use crate::config::VmConfig;
use crate::ffi::{
    exo_vm_create, exo_vm_free, exo_vm_is_running, exo_vm_last_error, exo_vm_start, exo_vm_stop,
    ExoVmHandle,
};
use crate::state::{VmState, VmStatus};
use std::ffi::{CStr, CString};
use std::ptr;
use tracing::{info, warn};

/// Manages a single Exo Linux microVM on macOS.
pub struct VmManager {
    config: VmConfig,
    state: VmState,
    handle: *mut ExoVmHandle,
    /// Host-loopback port tunnels into the guest, keyed by host port.
    tunnels: std::collections::HashMap<u16, crate::tunnel::HostTunnel>,
}

impl VmManager {
    pub fn new(config: VmConfig) -> anyhow::Result<Self> {
        let state = Self::load_or_create_state(&config)?;
        Ok(Self {
            config,
            state,
            handle: ptr::null_mut(),
            tunnels: std::collections::HashMap::new(),
        })
    }

    fn load_or_create_state(config: &VmConfig) -> anyhow::Result<VmState> {
        let state_path = crate::paths::state_path()?;
        if state_path.exists() {
            return Ok(VmState::load(&state_path)?);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let state = VmState::new(
            &config.name,
            &id,
            crate::paths::kernel_path()?,
            crate::paths::exo_initrd_path()?,
            crate::paths::disk_path()?,
        );
        state.save(&state_path)?;
        Ok(state)
    }

    /// Download/cache the guest image artifacts.
    pub async fn init(&self, force: bool) -> anyhow::Result<()> {
        info!("Initializing Exo microVM image");
        crate::builder::ensure_image(&self.config, force).await?;
        info!("Exo microVM image ready");
        Ok(())
    }

    /// Create and start the VM.
    pub fn start(&mut self, foreground: bool) -> anyhow::Result<()> {
        if !self.state.kernel_path.exists() || !self.state.initrd_path.exists() {
            anyhow::bail!("VM image not initialized; run 'exo vm init' first");
        }
        if !self.handle.is_null() {
            warn!("VM already has a handle; stopping before restart");
            let _ = self.stop(false);
        }

        let kernel = CString::new(self.state.kernel_path.to_string_lossy().as_bytes())?;
        let initrd = CString::new(self.state.initrd_path.to_string_lossy().as_bytes())?;
        // Attach the persistent-state disk when it exists; the guest init
        // degrades to ephemeral state when no block device is present.
        let disk_arg = if self.state.disk_path.exists() {
            self.state.disk_path.to_string_lossy().to_string()
        } else {
            warn!(
                "VM disk image missing at {}; booting without persistent state",
                self.state.disk_path.display()
            );
            String::new()
        };
        let disk = CString::new(disk_arg)?;
        let console_log = CString::new(
            crate::paths::logs_dir()?
                .join("vm-console.log")
                .to_string_lossy()
                .as_bytes(),
        )?;
        let name = CString::new(self.config.name.as_bytes())?;

        let handle = unsafe {
            exo_vm_create(
                kernel.as_ptr(),
                initrd.as_ptr(),
                disk.as_ptr(),
                console_log.as_ptr(),
                self.config.memory_bytes(),
                self.config.cpu_count,
                name.as_ptr(),
            )
        };
        if handle.is_null() {
            anyhow::bail!("failed to create VM: null handle");
        }
        self.handle = handle;

        let err = unsafe {
            CStr::from_ptr(exo_vm_last_error(self.handle))
                .to_string_lossy()
                .to_string()
        };
        if !err.is_empty() {
            unsafe { exo_vm_free(self.handle) };
            self.handle = ptr::null_mut();
            anyhow::bail!("failed to create VM: {}", err);
        }

        let ret = unsafe { exo_vm_start(self.handle) };
        if ret != 0 {
            let err = unsafe {
                CStr::from_ptr(exo_vm_last_error(self.handle))
                    .to_string_lossy()
                    .to_string()
            };
            unsafe { exo_vm_free(self.handle) };
            self.handle = ptr::null_mut();
            anyhow::bail!("failed to start VM: {}", err);
        }

        self.state.set_status(VmStatus::Running);
        self.state.started_at = Some(chrono::Utc::now());
        self.state.save(&crate::paths::state_path()?)?;
        info!("VM started");

        if foreground {
            info!("Running in foreground; press Ctrl+C to stop");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if !self.is_running() {
                    info!("VM stopped");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Stop the running VM and release the handle.
    pub fn stop(&mut self, _force: bool) -> anyhow::Result<()> {
        if self.handle.is_null() {
            anyhow::bail!("VM is not running");
        }
        // Tunnels pump vsock connections on this handle; stop them before the
        // VM handle is freed.
        self.tunnels.clear();
        let ret = unsafe { exo_vm_stop(self.handle) };
        if ret != 0 {
            let err = unsafe {
                CStr::from_ptr(exo_vm_last_error(self.handle))
                    .to_string_lossy()
                    .to_string()
            };
            warn!("VM stop returned error: {}; releasing handle anyway", err);
        }
        unsafe { exo_vm_free(self.handle) };
        self.handle = ptr::null_mut();

        self.state.set_status(VmStatus::Stopped);
        self.state.started_at = None;
        self.state.save(&crate::paths::state_path()?)?;
        info!("VM stopped");
        Ok(())
    }

    fn is_running(&self) -> bool {
        if self.handle.is_null() {
            return false;
        }
        unsafe { exo_vm_is_running(self.handle) != 0 }
    }

    /// Public running check for the control daemon.
    pub fn running(&self) -> bool {
        self.is_running()
    }

    /// How long ago the VM was started, if it is running. Used by the daemon
    /// to gate guest RPC until the guest agent has had time to boot: touching
    /// the serial RPC port too early can wedge the Virtualization.framework
    /// serial pump for the lifetime of the boot.
    pub fn boot_elapsed(&self) -> Option<std::time::Duration> {
        let started = self.state.started_at?;
        let now = chrono::Utc::now();
        let elapsed = now.signed_duration_since(started);
        if elapsed.num_seconds() < 0 {
            return Some(std::time::Duration::ZERO);
        }
        Some(std::time::Duration::from_secs(elapsed.num_seconds() as u64))
    }

    /// Send a request to the in-guest agent over the RPC serial port.
    pub fn guest_request(&self, req: GuestRequest) -> anyhow::Result<GuestResponse> {
        if self.handle.is_null() {
            anyhow::bail!("VM is not running");
        }
        agent_client::request(
            self.handle,
            self.config.vsock_port,
            req,
            self.config.guest_agent_timeout_ms,
        )
    }

    /// Publish a guest TCP port on the host loopback. The vsock connection is
    /// opened per accepted host connection, so this can be called before the
    /// guest agent is up; it only binds the host listener.
    pub fn start_tunnel(&mut self, host_port: u16, guest_port: u16) -> anyhow::Result<()> {
        if self.handle.is_null() {
            anyhow::bail!("VM is not running");
        }
        if self.tunnels.contains_key(&host_port) {
            anyhow::bail!("a tunnel is already bound to host port {}", host_port);
        }
        let tunnel = crate::tunnel::HostTunnel::start(
            crate::tunnel::SendableHandle(self.handle),
            host_port,
            guest_port,
        )?;
        self.tunnels.insert(host_port, tunnel);
        info!("tunnel: 127.0.0.1:{} -> guest :{}", host_port, guest_port);
        Ok(())
    }

    /// Stop the tunnel bound to a host port, if any.
    pub fn stop_tunnel(&mut self, host_port: u16) -> bool {
        self.tunnels.remove(&host_port).is_some()
    }

    /// Print VM status.
    pub fn status(&self, json: bool) -> anyhow::Result<()> {
        let running = if self.handle.is_null() {
            false
        } else {
            unsafe { exo_vm_is_running(self.handle) != 0 }
        };

        let mut guest_ok = false;
        let mut guest_info = String::new();
        if running {
            match agent_client::request(
                self.handle,
                self.config.vsock_port,
                GuestRequest::Ping,
                self.config.guest_agent_timeout_ms,
            ) {
                Ok(GuestResponse::Pong) => {
                    guest_ok = true;
                    guest_info = "agent responded".to_string();
                }
                Ok(other) => {
                    guest_info = format!("unexpected response: {:?}", other);
                }
                Err(e) => {
                    guest_info = format!("agent unreachable: {}", e);
                }
            }
        }

        if json {
            let obj = serde_json::json!({
                "name": self.state.name,
                "status": if running { "running" } else { "stopped" },
                "guest_agent_reachable": guest_ok,
                "guest_agent_info": guest_info,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("VM: {}", self.state.name);
            println!("Status: {}", if running { "running" } else { "stopped" });
            println!(
                "Guest agent: {}",
                if guest_ok { "reachable" } else { "unreachable" }
            );
            if !guest_info.is_empty() {
                println!("Guest info: {}", guest_info);
            }
        }
        Ok(())
    }

    /// Reset the VM image and optionally clear runtime state.
    pub async fn reset(&mut self, keep_state: bool) -> anyhow::Result<()> {
        if !self.handle.is_null() {
            let _ = self.stop(false);
        }
        if !keep_state {
            let state_path = crate::paths::state_path()?;
            if state_path.exists() {
                std::fs::remove_file(&state_path)?;
                self.state = Self::load_or_create_state(&self.config)?;
            }
        }
        crate::builder::ensure_image(&self.config, true).await?;
        info!("VM reset complete");
        Ok(())
    }
}

impl Drop for VmManager {
    fn drop(&mut self) {
        // Tunnel threads call into the VM handle; they must be joined before
        // the handle is stopped and freed.
        self.tunnels.clear();
        if !self.handle.is_null() {
            warn!("VmManager dropped with live VM; stopping");
            unsafe {
                let _ = exo_vm_stop(self.handle);
                exo_vm_free(self.handle);
            }
            self.handle = ptr::null_mut();
        }
    }
}
