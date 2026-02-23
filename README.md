# Containment - AI Agent Container Runtime

A custom container runtime for AI agents, deployable on Windows using WSL2 backend.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Containment (Windows .exe)                │
├─────────────────────────────────────────────────────────────┤
│  CLI                                                          │
│  - run, ps, stop, logs, exec commands                        │
│  - TOML config parsing                                       │
├─────────────────────────────────────────────────────────────┤
│  Container Runtime Core (Cross-Platform)                     │
│  - Container lifecycle management                            │
│  - Image handling                                            │
│  - GPU configuration                                         │
├─────────────────────────────────────────────────────────────┤
│  WSL2 Backend (Windows-specific)                            │
│  - Manages WSL2 distro for containers                        │
│  - Executes container commands inside WSL2                   │
│  - Filesystem mounting (Windows ↔ WSL2)                     │
│  - GPU passthrough to WSL2                                  │
├─────────────────────────────────────────────────────────────┤
│  Windows GPU Detection                                      │
│  - Detects NVIDIA/AMD GPUs on Windows                       │
│  - Validates WSL2 GPU support                               │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    WSL2 (Lightweight VM)                     │
│  - Real Linux kernel                                        │
│  - Near-native performance (~95-98%)                        │
│  - GPU passthrough supported                                │
│  - Runs Containment Linux runtime (unshare, namespaces)     │
└─────────────────────────────────────────────────────────────┘
```

## Project Structure

```
containment/
├── Cargo.toml                    # Workspace
├── crates/
│   ├── cli/                      # Windows CLI application
│   │   ├── src/
│   │   │   ├── main.rs           # Entry point
│   │   │   └── commands/         # CLI commands
│   │   └── Cargo.toml
│   ├── runtime/                  # Core container runtime
│   │   ├── src/
│   │   │   ├── container.rs      # Container abstraction
│   │   │   ├── config.rs         # Configuration parsing
│   │   │   └── platform/         # Platform-specific backends
│   │   └── Cargo.toml
│   ├── wsl/                      # WSL2 management (Windows only)
│   │   ├── src/
│   │   │   ├── distro.rs         # WSL2 distro management
│   │   │   ├── command.rs        # Execute commands in WSL2
│   │   │   ├── mount.rs          # Windows↔WSL2 filesystem
│   │   │   └── gpu.rs            # GPU passthrough to WSL2
│   │   └── Cargo.toml
│   ├── gpu/                      # GPU detection (cross-platform)
│   │   ├── src/
│   │   │   ├── windows.rs        # Windows GPU detection
│   │   │   └── linux.rs          # Linux GPU detection
│   │   └── Cargo.toml
│   └── image/                    # OCI image handling
│       └── ...
│   ├── linux-runtime/            # Linux runtime that runs inside WSL2
│   │   ├── Cargo.toml            # Linux binary built separately
│   │   ├── src/
│   │   │   ├── main.rs           # Entry point for WSL2
│   │   │   ├── namespaces.rs     # Linux namespace operations
│   │   │   ├── cgroup.rs         # cgroups v2 setup
│   │   │   ├── mount.rs          # Mount operations
│   │   │   ├── process.rs       # Fork+exec with namespaces
│   │   │   ├── container.rs     # Container lifecycle
│   │   │   ├── rootfs.rs        # Root filesystem setup
│   │   │   ├── state.rs         # State persistence
│   │   │   └── server.rs        # RPC server
│   │   └── build.sh             # Build Linux binary
│   └── installer/                # First-run setup
│       └── setup.rs               # WSL2 installation/check
└── examples/                    # Sample agent configurations
    ├── python-agent.toml
    ├── nodejs-agent.toml
    └── gpu-agent.toml
```

## Usage

```bash
# Run an AI agent with GPU
containment run --gpu python:3.12 -- python train.py

# Use config file
containment run -f examples/gpu-agent.toml

# Use Windows folders
containment run -v C:\mycode:/app python:3.12 -- python script.py

# List containers
containment ps

# Stop container
containment stop <id>
```

## Configuration Example

```toml
[container]
name = "gpu-agent"
image = "nvidia/cuda:12.1.0-runtime-ubuntu22.04"

[container.resources]
memory = "8G"
cpu = "4"

[container.runtime]
workdir = "/workspace"
env = ["CUDA_VISIBLE_DEVICES=0"]

[container.network]
mode = "bridge"

[container.gpu]
enabled = true
type = "nvidia"
devices = "all"

[process]
command = ["python", "train_model.py"]
```

## Deployment

Single `containment.exe` that:
1. Checks for WSL2 installation on first run
2. Installs/creates 'containment' WSL2 distro
3. Deploys runtime binary into WSL2
4. Ready to run containers
