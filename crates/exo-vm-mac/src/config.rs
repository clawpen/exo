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

/// Optional TOML file representation of VmConfig; every field defaults to the
/// built-in value when absent.
#[derive(Debug, Default, Deserialize)]
struct VmConfigFile {
    name: Option<String>,
    kernel_url: Option<String>,
    initrd_url: Option<String>,
    cpu_count: Option<u32>,
    memory_mb: Option<u64>,
    vsock_port: Option<u32>,
    guest_agent_timeout_ms: Option<u32>,
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
            // 1 GiB is demonstrably tight once a node agent + npm share the
            // VM; 2 GiB covers a few concurrent agent containers.
            memory_mb: 2048,
            vsock_port: 1024,
            // Large guest-side operations (image extraction, workspace export)
            // can take tens of seconds; keep the per-request timeout above them.
            guest_agent_timeout_ms: 60_000,
        }
    }
}

impl VmConfig {
    pub fn memory_bytes(&self) -> u64 {
        self.memory_mb * 1024 * 1024
    }

    /// Path of the optional user config file.
    pub fn config_path() -> Option<std::path::PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return Some(std::path::PathBuf::from(xdg).join("exo").join("vm.toml"));
        }
        std::env::var("HOME")
            .ok()
            .map(|home| std::path::PathBuf::from(home).join(".config/exo/vm.toml"))
    }

    /// Load VM configuration: built-in defaults, overridden by
    /// `$XDG_CONFIG_HOME/exo/vm.toml` (or `~/.config/exo/vm.toml`) when
    /// present, then by `EXO_VM_CPUS` / `EXO_VM_MEMORY_MB` environment
    /// variables. Invalid files are ignored with a warning so a typo never
    /// blocks the VM from booting. Changes take effect on the next VM start.
    pub fn load() -> Self {
        let mut config = Self::default();
        if let Some(path) = Self::config_path() {
            if path.exists() {
                match std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|text| Ok(toml::from_str::<VmConfigFile>(&text)?))
                {
                    Ok(file) => config.apply_file(file),
                    Err(e) => {
                        tracing::warn!("ignoring invalid {}: {}", path.display(), e)
                    }
                }
            }
        }
        config.apply_env();
        config
    }

    fn apply_file(&mut self, file: VmConfigFile) {
        if let Some(name) = file.name {
            self.name = name;
        }
        if let Some(kernel_url) = file.kernel_url {
            self.kernel_url = kernel_url;
        }
        if let Some(initrd_url) = file.initrd_url {
            self.initrd_url = initrd_url;
        }
        if let Some(cpu_count) = file.cpu_count {
            self.cpu_count = cpu_count;
        }
        if let Some(memory_mb) = file.memory_mb {
            self.memory_mb = memory_mb;
        }
        if let Some(vsock_port) = file.vsock_port {
            self.vsock_port = vsock_port;
        }
        if let Some(guest_agent_timeout_ms) = file.guest_agent_timeout_ms {
            self.guest_agent_timeout_ms = guest_agent_timeout_ms;
        }
    }

    fn apply_env(&mut self) {
        if let Ok(cpus) = std::env::var("EXO_VM_CPUS") {
            match cpus.parse::<u32>() {
                Ok(cpus) if cpus > 0 => self.cpu_count = cpus,
                _ => tracing::warn!("ignoring invalid EXO_VM_CPUS={:?}", cpus),
            }
        }
        if let Ok(memory) = std::env::var("EXO_VM_MEMORY_MB") {
            match memory.parse::<u64>() {
                Ok(memory) if memory >= 256 => self.memory_mb = memory,
                _ => tracing::warn!("ignoring invalid EXO_VM_MEMORY_MB={:?}", memory),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_overrides_defaults() {
        let mut config = VmConfig::default();
        std::env::set_var("EXO_VM_CPUS", "6");
        std::env::set_var("EXO_VM_MEMORY_MB", "8192");
        config.apply_env();
        assert_eq!(config.cpu_count, 6);
        assert_eq!(config.memory_mb, 8192);
        std::env::remove_var("EXO_VM_CPUS");
        std::env::remove_var("EXO_VM_MEMORY_MB");
    }

    #[test]
    fn invalid_env_values_are_ignored() {
        let mut config = VmConfig::default();
        std::env::set_var("EXO_VM_CPUS", "zero");
        std::env::set_var("EXO_VM_MEMORY_MB", "12");
        config.apply_env();
        assert_eq!(config.cpu_count, 2);
        assert_eq!(config.memory_mb, 2048);
        std::env::remove_var("EXO_VM_CPUS");
        std::env::remove_var("EXO_VM_MEMORY_MB");
    }

    #[test]
    fn toml_file_overrides_selected_fields() {
        let mut config = VmConfig::default();
        let file: VmConfigFile = toml::from_str("cpu_count = 8\nmemory_mb = 6144").unwrap();
        config.apply_file(file);
        assert_eq!(config.cpu_count, 8);
        assert_eq!(config.memory_mb, 6144);
        assert_eq!(config.vsock_port, 1024);
    }
}
