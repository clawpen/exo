//! Foreign binary support via binfmt_misc and QEMU user-mode.
//!
//! This module enables running binaries for non-native architectures (e.g., ARM
//! on x86) by registering binfmt_misc handlers that use QEMU user-mode emulation.
//!
//! # Supported Architectures
//!
//! - `arm` - ARM 32-bit (requires qemu-arm)
//! - `aarch64` / `arm64` - ARM 64-bit (requires qemu-aarch64)
//! - `riscv64` - RISC-V 64-bit (requires qemu-riscv64)
//! - `ppc64le` - PowerPC 64-bit little-endian (requires qemu-ppc64le)
//! - `s390x` - IBM Z (requires qemu-s390x)
//! - `x86_64` - AMD64 (on non-x86 hosts, requires qemu-x86_64)
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! use exo_runtime::binfmt::{register_binfmt, is_qemu_available, Architecture};
//!
//! // Check if QEMU is available for ARM64
//! if is_qemu_available(Architecture::Aarch64) {
//!     register_binfmt(Architecture::Aarch64)?;
//! }
//! # Ok(())
//! # }
//! ```

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supported foreign architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Architecture {
    /// ARM 32-bit
    Arm,

    /// AArch64 / ARM64
    Aarch64,

    /// RISC-V 64-bit
    Riscv64,

    /// PowerPC 64-bit little-endian
    Ppc64Le,

    /// IBM Z / s390x
    S390x,

    /// x86_64 / AMD64
    X86_64,

    /// MIPS 64-bit little-endian
    Mips64Le,

    /// 64-bit (generic alias)
    Amd64,
}

impl Architecture {
    /// Get the QEMU binary name for this architecture.
    pub fn qemu_binary(&self) -> &str {
        match self {
            Architecture::Arm => "qemu-arm",
            Architecture::Aarch64 => "qemu-aarch64",
            Architecture::Riscv64 => "qemu-riscv64",
            Architecture::Ppc64Le => "qemu-ppc64le",
            Architecture::S390x => "qemu-s390x",
            Architecture::X86_64 | Architecture::Amd64 => "qemu-x86_64",
            Architecture::Mips64Le => "qemu-mips64el",
        }
    }

    /// Get the magic bytes for ELF identification as a byte string.
    pub fn magic_bytes(&self) -> &'static [u8] {
        match self {
            Architecture::Arm => {
                b"\x7fELF\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x28\x00"
            }
            Architecture::Aarch64 => {
                b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xb7\x00"
            }
            Architecture::Riscv64 => {
                b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xf3\x00"
            }
            Architecture::Ppc64Le => {
                b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x15\x00"
            }
            Architecture::S390x => {
                b"\x7fELF\x02\x02\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x16"
            }
            Architecture::X86_64 | Architecture::Amd64 => {
                b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x3e\x00"
            }
            Architecture::Mips64Le => {
                b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x08\x00"
            }
        }
    }

    /// Get the ELF mask for binfmt_misc as a byte string.
    pub fn elf_mask(&self) -> &'static [u8] {
        match self {
            Architecture::Arm => b"\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
            Architecture::Aarch64 => b"\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
            Architecture::Riscv64 => b"\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
            Architecture::Ppc64Le => b"\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xfe",
            Architecture::S390x => b"\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
            Architecture::X86_64 | Architecture::Amd64 => b"\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
            Architecture::Mips64Le => b"\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
        }
    }

    /// Get the binfmt_misc registration name.
    pub fn binfmt_name(&self) -> &str {
        match self {
            Architecture::Arm => "qemu-arm",
            Architecture::Aarch64 => "qemu-aarch64",
            Architecture::Riscv64 => "qemu-riscv64",
            Architecture::Ppc64Le => "qemu-ppc64le",
            Architecture::S390x => "qemu-s390x",
            Architecture::X86_64 | Architecture::Amd64 => "qemu-x86_64",
            Architecture::Mips64Le => "qemu-mips64el",
        }
    }

    /// Parse from architecture string.
    pub fn from_str(s: &str) -> Option<Architecture> {
        match s.to_lowercase().as_str() {
            "arm" | "armv7" | "armv7l" => Some(Architecture::Arm),
            "aarch64" | "arm64" | "armv8" => Some(Architecture::Aarch64),
            "riscv64" | "riscv" => Some(Architecture::Riscv64),
            "ppc64le" | "powerpc64le" => Some(Architecture::Ppc64Le),
            "s390x" => Some(Architecture::S390x),
            "x86_64" | "amd64" | "x64" => Some(Architecture::X86_64),
            "mips64le" | "mips64el" => Some(Architecture::Mips64Le),
            _ => None,
        }
    }

    /// Detect the host architecture.
    pub fn host_arch() -> Architecture {
        use std::env::consts::ARCH;

        match ARCH {
            "arm" => Architecture::Arm,
            "aarch64" => Architecture::Aarch64,
            "riscv64" => Architecture::Riscv64,
            "powerpc64" => Architecture::Ppc64Le,
            "s390x" => Architecture::S390x,
            "x86_64" => Architecture::X86_64,
            "mips64" => Architecture::Mips64Le,
            _ => Architecture::X86_64, // Default fallback
        }
    }

    /// Check if this architecture is foreign (non-native).
    pub fn is_foreign(&self) -> bool {
        *self != Self::host_arch()
    }
}

/// Check if QEMU user-mode emulator is available for an architecture.
pub fn is_qemu_available(arch: Architecture) -> bool {
    let qemu_bin = arch.qemu_binary();

    // Check if binary exists in PATH
    Command::new(qemu_bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get all available QEMU user-mode emulators.
pub fn available_qemu_targets() -> Vec<Architecture> {
    vec![
        Architecture::Arm,
        Architecture::Aarch64,
        Architecture::Riscv64,
        Architecture::Ppc64Le,
        Architecture::S390x,
        Architecture::X86_64,
        Architecture::Mips64Le,
    ]
    .into_iter()
    .filter(|arch| is_qemu_available(*arch))
    .collect()
}

/// Path to binfmt_misc mount point.
pub const BINFMT_MISC_PATH: &str = "/proc/sys/fs/binfmt_misc";

/// Check if binfmt_misc is available and mounted.
pub fn is_binfmt_available() -> bool {
    Path::new(BINFMT_MISC_PATH).exists() || Path::new("/sys/fs/binfmt_misc").exists()
}

/// Mount binfmt_misc if not already mounted.
pub fn ensure_binfmt_mounted() -> Result<()> {
    if is_binfmt_available() {
        return Ok(());
    }

    // Try to mount binfmt_misc
    let status = Command::new("mount")
        .args(["-t", "binfmt_misc", "none", "/proc/sys/fs/binfmt_misc"])
        .status()?;

    if status.success() {
        tracing::info!("Mounted binfmt_misc filesystem");
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to mount binfmt_misc"))
    }
}

/// Register a binfmt_misc handler for QEMU emulation.
///
/// This writes to /proc/sys/fs/binfmt_misc/register to enable transparent
/// emulation of foreign binaries.
///
/// # Arguments
///
/// * `arch` - The target architecture to register
///
/// # Example
///
/// ```no_run
/// # fn main() -> anyhow::Result<()> {
/// use exo_runtime::binfmt::{register_binfmt, Architecture};
///
/// // Enable ARM64 binary execution
/// register_binfmt(Architecture::Aarch64)?;
/// # Ok(())
/// # }
/// ```
pub fn register_binfmt(arch: Architecture) -> Result<()> {
    ensure_binfmt_mounted()?;

    let qemu_path = find_qemu_binary(arch.qemu_binary())
        .context(format!("QEMU binary {} not found", arch.qemu_binary()))?;

    let binfmt_name = arch.binfmt_name();
    let register_path = PathBuf::from(BINFMT_MISC_PATH).join("register");

    // Build registration string
    // Format: :name:magic:mask:interpreter:flags
    // Magic and mask need to be hex-encoded
    let magic = arch.magic_bytes();
    let mask = arch.elf_mask();

    // Convert byte arrays to hex strings for binfmt_misc
    let magic_hex: String = magic.iter().map(|b| format!("{:02x}", b)).collect();

    let mask_hex: String = mask.iter().map(|b| format!("{:02x}", b)).collect();

    let registration = format!(
        ":{}:{}:{}:{}:OC",
        binfmt_name,
        magic_hex,
        mask_hex,
        qemu_path.display()
    );

    fs::write(&register_path, registration)
        .with_context(|| format!("Failed to register binfmt for {:?}", arch))?;

    tracing::info!(
        "Registered binfmt handler for {:?}: {}",
        arch,
        qemu_path.display()
    );

    Ok(())
}

/// Unregister a binfmt_misc handler.
pub fn unregister_binfmt(arch: Architecture) -> Result<()> {
    let binfmt_name = arch.binfmt_name();
    let unregister_path = PathBuf::from(BINFMT_MISC_PATH).join(binfmt_name);

    // Writing 0 to the file unregisters it
    fs::write(&unregister_path, "0")
        .with_context(|| format!("Failed to unregister binfmt for {:?}", arch))?;

    tracing::info!("Unregistered binfmt handler for {:?}", arch);

    Ok(())
}

/// Find the full path to a QEMU binary.
pub fn find_qemu_binary(name: &str) -> Option<PathBuf> {
    // Common locations to check
    let paths = vec![
        format!("/usr/bin/{}", name),
        format!("/usr/local/bin/{}", name),
        format!("/bin/{}", name),
    ];

    for path in paths {
        if Path::new(&path).exists() {
            return Some(PathBuf::from(path));
        }
    }

    // Check via PATH
    if let Ok(output) = Command::new("which").arg(name).output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }

    None
}

/// Register all available QEMU targets.
///
/// This is useful for enabling multi-arch container support.
pub fn register_all_available() -> Result<Vec<Architecture>> {
    let available = available_qemu_targets();
    let mut registered = Vec::new();

    for arch in available {
        if arch.is_foreign() {
            match register_binfmt(arch) {
                Ok(_) => registered.push(arch),
                Err(e) => {
                    tracing::warn!("Failed to register {:?}: {}", arch, e);
                }
            }
        }
    }

    Ok(registered)
}

/// Check if a specific binfmt handler is registered.
pub fn is_binfmt_registered(arch: Architecture) -> bool {
    let binfmt_name = arch.binfmt_name();
    let status_path = PathBuf::from(BINFMT_MISC_PATH).join(binfmt_name);

    status_path.exists()
}

/// Get status of a binfmt handler.
pub fn get_binfmt_status(arch: Architecture) -> Option<BinfmtStatus> {
    let binfmt_name = arch.binfmt_name();
    let status_path = PathBuf::from(BINFMT_MISC_PATH).join(binfmt_name);

    if !status_path.exists() {
        return None;
    }

    if let Ok(content) = fs::read_to_string(&status_path) {
        BinfmtStatus::from_status_str(&content)
    } else {
        None
    }
}

/// Status of a binfmt_misc handler.
#[derive(Debug, Clone)]
pub struct BinfmtStatus {
    /// Whether the handler is enabled
    pub enabled: bool,
    /// The interpreter binary path
    pub interpreter: PathBuf,
    /// Additional flags
    pub flags: Vec<String>,
}

impl BinfmtStatus {
    fn from_status_str(s: &str) -> Option<Self> {
        let lines: Vec<&str> = s.lines().collect();
        if lines.is_empty() {
            return None;
        }

        // First line is: enabled\n
        let enabled = lines[0].trim() == "1";

        // Second line is: interpreter/path
        let interpreter = if lines.len() > 1 {
            PathBuf::from(lines[1].trim())
        } else {
            return None;
        };

        // Remaining lines are flags
        let flags = if lines.len() > 2 {
            lines[2..].iter().map(|s| s.trim().to_string()).collect()
        } else {
            vec![]
        };

        Some(BinfmtStatus {
            enabled,
            interpreter,
            flags,
        })
    }
}

/// Detect the architecture of an ELF binary.
///
/// Reads the ELF header to determine the target architecture.
pub fn detect_binary_arch(path: &Path) -> Option<Architecture> {
    if !path.exists() {
        return None;
    }

    // Read first 64 bytes for ELF identification
    let mut buffer = [0u8; 64];
    if let Ok(mut file) = std::fs::File::open(path) {
        use std::io::Read;
        if file.read_exact(&mut buffer).is_err() {
            return None;
        }
    } else {
        return None;
    }

    // Check ELF magic
    if &buffer[0..4] != b"\x7fELF" {
        return None;
    }

    // Get architecture from e_machine field (offset 18-19)
    // and endianness/class from offset 4-5
    let is_64bit = buffer[4] == 2;
    let is_little_endian = buffer[5] == 1;

    // e_machine is at offset 16-17 (16-bit)
    let e_machine = u16::from_le_bytes([buffer[18], buffer[19]]);

    match (e_machine, is_64bit, is_little_endian) {
        (40, false, _) => Some(Architecture::Arm),       // EM_ARM
        (62, true, _) => Some(Architecture::X86_64),     // EM_X86_64
        (183, true, _) => Some(Architecture::Aarch64),   // EM_AARCH64
        (243, true, _) => Some(Architecture::Riscv64),   // EM_RISCV
        (21, true, true) => Some(Architecture::Ppc64Le), // EM_PPC64 (little endian)
        (22, true, false) => Some(Architecture::S390x),  // EM_S390
        (8, true, true) => Some(Architecture::Mips64Le), // EM_MIPS
        _ => None,
    }
}

/// Set up foreign binary execution for a container.
///
/// This function registers binfmt handlers for the specified architecture
/// before spawning a container, enabling transparent execution of foreign
/// binaries inside the container.
///
/// # Arguments
///
/// * `arch` - Target architecture for the container
pub fn setup_foreign_exec(arch: Architecture) -> Result<()> {
    if !arch.is_foreign() {
        tracing::debug!("Architecture {:?} is native, skipping binfmt setup", arch);
        return Ok(());
    }

    if !is_qemu_available(arch) {
        anyhow::bail!(
            "QEMU user-mode emulator not available for {:?}. \
            Install {} package.",
            arch,
            arch.qemu_binary()
        );
    }

    if is_binfmt_registered(arch) {
        tracing::debug!("binfmt handler for {:?} already registered", arch);
        return Ok(());
    }

    register_binfmt(arch)?;

    tracing::info!("Set up foreign binary execution for {:?}", arch);

    Ok(())
}

/// Clean up binfmt registrations.
///
/// Unregisters all QEMU binfmt handlers that were registered.
pub fn cleanup_binfmt() -> Result<()> {
    let archs = available_qemu_targets();

    for arch in archs {
        if is_binfmt_registered(arch) {
            let _ = unregister_binfmt(arch);
        }
    }

    Ok(())
}

/// Check if binfmt_misc support is working by testing registration.
pub fn test_binfmt_support() -> Result<bool> {
    if !is_binfmt_available() {
        ensure_binfmt_mounted()?;
    }

    // Try to read the register file
    let register_path = PathBuf::from(BINFMT_MISC_PATH).join("register");
    if register_path.exists() {
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_architecture_qemu_binary() {
        assert_eq!(Architecture::Arm.qemu_binary(), "qemu-arm");
        assert_eq!(Architecture::Aarch64.qemu_binary(), "qemu-aarch64");
        assert_eq!(Architecture::X86_64.qemu_binary(), "qemu-x86_64");
    }

    #[test]
    fn test_architecture_from_str() {
        assert_eq!(Architecture::from_str("arm"), Some(Architecture::Arm));
        assert_eq!(
            Architecture::from_str("aarch64"),
            Some(Architecture::Aarch64)
        );
        assert_eq!(Architecture::from_str("arm64"), Some(Architecture::Aarch64));
        assert_eq!(Architecture::from_str("x86_64"), Some(Architecture::X86_64));
        assert_eq!(
            Architecture::from_str("riscv64"),
            Some(Architecture::Riscv64)
        );
        assert_eq!(Architecture::from_str("unknown"), None);
    }

    #[test]
    fn test_architecture_binfmt_name() {
        assert_eq!(Architecture::Arm.binfmt_name(), "qemu-arm");
        assert_eq!(Architecture::Aarch64.binfmt_name(), "qemu-aarch64");
    }

    #[test]
    fn test_host_arch() {
        let host = Architecture::host_arch();
        // Should return a valid architecture
        assert!(!host.is_foreign());
    }

    #[test]
    fn test_foreign_detection() {
        let host = Architecture::host_arch();

        // Create a definitely different architecture
        let foreign = match host {
            Architecture::X86_64 => Architecture::Aarch64,
            Architecture::Aarch64 => Architecture::X86_64,
            _ => Architecture::X86_64,
        };

        assert!(foreign.is_foreign());
        assert!(!host.is_foreign());
    }
}
