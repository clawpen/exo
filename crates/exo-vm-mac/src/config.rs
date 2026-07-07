use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub name: String,
    pub kernel_url: String,
    pub initrd_url: String,
    pub cpu_count: u32,
    pub memory_mb: u64,
    pub vsock_port: u32,
    pub guest_agent_timeout_ms: u32,
}

impl Default for VmConfig {
    fn default() -> Self {
        // Debian installer netboot provides a raw ARM64 Linux Image (not an EFI PE
        // executable), which is what Apple's VZLinuxBootLoader expects.
        let base = "https://deb.debian.org/debian/dists/stable/main/installer-arm64/current/images/netboot/debian-installer/arm64";
        Self {
            name: "exo-vm".to_string(),
            kernel_url: format!("{base}/linux"),
            initrd_url: format!("{base}/initrd.gz"),
            cpu_count: 2,
            memory_mb: 1024,
            vsock_port: 1024,
            guest_agent_timeout_ms: 5000,
        }
    }
}

impl VmConfig {
    pub fn memory_bytes(&self) -> u64 {
        self.memory_mb * 1024 * 1024
    }
}
