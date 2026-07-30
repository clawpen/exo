use std::path::PathBuf;

pub fn exo_vm_dir() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = std::env::var("EXO_STATE_DIR") {
        let dir = PathBuf::from(dir).join("vm");
        std::fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        let dir = PathBuf::from(xdg).join("exo").join("vm");
        std::fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        let dir = PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("exo")
            .join("vm");
        std::fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    #[cfg(unix)]
    {
        let fallback =
            std::env::temp_dir().join(format!("exo-vm-uid-{}", unsafe { libc::getuid() }));
        std::fs::create_dir_all(&fallback)?;
        return Ok(fallback);
    }
    #[cfg(not(unix))]
    {
        let fallback = std::env::temp_dir().join("exo-vm");
        std::fs::create_dir_all(&fallback)?;
        return Ok(fallback);
    }
}

pub fn kernel_dir() -> anyhow::Result<PathBuf> {
    let dir = exo_vm_dir()?.join("kernel");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn initrd_dir() -> anyhow::Result<PathBuf> {
    let dir = exo_vm_dir()?.join("initrd");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn logs_dir() -> anyhow::Result<PathBuf> {
    let dir = exo_vm_dir()?.join("logs");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn kernel_path() -> anyhow::Result<PathBuf> {
    Ok(kernel_dir()?.join("vmlinuz-lts"))
}

pub fn base_initrd_path() -> anyhow::Result<PathBuf> {
    Ok(initrd_dir()?.join("initramfs-lts"))
}

pub fn exo_initrd_path() -> anyhow::Result<PathBuf> {
    Ok(initrd_dir()?.join("initrd-exo.img"))
}

pub fn disk_path() -> anyhow::Result<PathBuf> {
    Ok(exo_vm_dir()?.join("disk.img"))
}

pub fn state_path() -> anyhow::Result<PathBuf> {
    Ok(exo_vm_dir()?.join("vm.json"))
}

pub fn guest_agent_binary_path() -> anyhow::Result<PathBuf> {
    Ok(exo_vm_dir()?.join("guest-agent"))
}

pub fn exo_agent_binary_path() -> anyhow::Result<PathBuf> {
    Ok(exo_vm_dir()?.join("exo-agent"))
}

pub fn control_socket_path() -> anyhow::Result<PathBuf> {
    Ok(exo_vm_dir()?.join("exo-vm.sock"))
}

pub fn daemon_log_path() -> anyhow::Result<PathBuf> {
    Ok(logs_dir()?.join("vm-daemon.log"))
}
