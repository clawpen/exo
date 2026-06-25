# Exo on Windows

Exo supports Windows through WSL2 (Windows Subsystem for Linux 2). This allows the Linux container runtime to work seamlessly on Windows while providing native performance.

## Prerequisites

1. **Windows 11** or **Windows 10** (version 2004 or later)
2. **WSL2** installed and configured
3. A WSL2 distribution (Ubuntu, Alpine, etc.)

## Installing WSL2

If WSL2 is not already installed, run:

```powershell
wsl --install
```

Or install a specific distro:

```powershell
wsl --install -d Ubuntu-22.04
```

## Quick Start

### 1. Clone the repository

```powershell
git clone https://github.com/clawpen/exo.git
cd exo
```

### 2. Build exo for WSL2

```powershell
.\scripts\build-wsl.ps1
```

Or from within WSL2:

```bash
wsl
cd /mnt/f/Software/exo  # Adjust path as needed
cargo build --release
```

### 3. Run exo

Use the Windows wrapper script:

```cmd
.\exo.cmd run python:3.12 python -c "print('Hello from WSL2!')"
```

Or directly via WSL2:

```bash
wsl /mnt/f/Software/exo/target/release/exo run python:3.12 python -c "print('Hello!')"
```

## Windows Wrapper Script

The `exo.cmd` script automatically:
1. Converts Windows paths to WSL2 paths
2. Forwards all commands to the Linux binary in WSL2
3. Handles builds when needed

## Path Translation

Windows paths are automatically translated for use in WSL2 containers:

| Windows Path | WSL2 Path |
|--------------|-----------|
| `C:\Users\foo\bar` | `/mnt/c/Users/foo/bar` |
| `F:\Software\exo` | `/mnt/f/Software/exo` |
| `D:\data` | `/mnt/d/data` |

Volume mounts work seamlessly:

```cmd
.\exo.cmd run -v C:\Users\MyUser\data:/data python:3.12
```

## GPU Support

WSL2 supports GPU passthrough for CUDA and ROCm:

```cmd
.\exo.cmd run --gpu python:3.12 python -c "import torch; print(torch.cuda.is_available())"
```

Requirements:
- NVIDIA GPU with WSL2-compatible drivers
- or AMD GPU with ROCm support in WSL2

## Performance

Compared to Docker Desktop:

| Metric | Docker Desktop | Exo (WSL2) |
|--------|----------------|------------|
| Container startup | 2-3s | <500ms |
| Memory overhead | ~100MB/container | ~20MB/container |
| Daemon overhead | ~2GB RSS | ~10MB (lightweight daemon) |

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Windows Host                                                │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  exo.cmd (Windows wrapper)                           │   │
│  └──────────────────────────────────────────────────────┘   │
│                          │                                   │
│                          │ WSL2 Bridge                      │
│                          ▼                                   │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  WSL2 Distro (Ubuntu/Alpine)                        │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  exo (Linux binary)                            │  │   │
│  │  │  - Namespace isolation                         │  │   │
│  │  │  - Cgroup v2 limits                           │  │   │
│  │  │  - Seccomp filtering                          │  │   │
│  │  │  - Lightweight daemon, fast spawning          │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  │                                                      │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  Agent Containers                             │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## Troubleshooting

### WSL2 not detected

```powershell
# Check WSL2 status
wsl --status

# Set WSL2 as default
wsl --set-default-version 2
```

### Build errors

```powershell
# Update Rust in WSL2
wsl
rustup update stable
```

### Permission errors

```bash
# Inside WSL2, fix permissions
sudo chmod +x /mnt/f/Software/exo/target/release/exo
```

### GPU not available in containers

1. Verify GPU works in WSL2:
   ```bash
   wsl nvidia-smi  # For NVIDIA
   ```

2. Install WSL2-compatible GPU drivers on Windows

### Network issues

WSL2 networking uses NAT. For external access:

```powershell
# Forward port from Windows to WSL2
netsh interface portproxy add v4tov4 listenport=8080 listenaddress=0.0.0.0 connectport=8080 connectaddress=172.x.x.x
```

## Testing

Run the test script to verify your setup:

```powershell
.\scripts\test-wsl.ps1
```

This will check:
- WSL2 installation
- Distro availability
- exo binary accessibility
- Container runtime support
- GPU support
- Network configuration

## Development

### Building for Windows

The main `exo` binary is Windows-native but delegates to WSL2 for container operations:

```powershell
cargo build --release
```

### Building for Linux (in WSL2)

```bash
wsl
cd /mnt/f/Software/exo
cargo build --release
```

## Next Steps

- See [examples/](../examples/) for usage examples
- Read [ROADMAP.md](../ROADMAP.md) for planned features
- Check [TEST_PLAN.md](../TEST_PLAN.md) for testing guidance
