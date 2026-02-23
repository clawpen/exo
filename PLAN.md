# Containment Container Runtime

A lightweight, Linux-first container runtime optimized for AI agent workloads, deployable on Windows via WSL2.

## Quick Start

```bash
# Run an AI agent with GPU
containment run --gpu python:3.12 -- python train.py

# Use config file
containment run -f examples/gpu-agent.toml

# List containers
containment ps
```

## Architecture

```
Windows (containment.exe)
    │
    ├── Detects GPUs on Windows (nvidia-smi)
    ├── Manages WSL2 'containment' distro
    ├── Converts paths (C:\path → /mnt/c/path)
    └── Executes commands via: wsl -d containment ...
```

## Project Structure

```
├── Cargo.toml                    # Workspace
├── crates/
│   ├── cli/                     # containment.exe
│   ├── containment-runtime/    # Shared types
│   ├── containment-gpu/         # GPU detection
│   ├── containment-image/       # OCI images
│   └── containment-wsl/         # WSL2 backend
├── containment-runtime/         # Linux binary for WSL2
└── examples/                    # Config files
```

## Configuration

```toml
[container]
name = "gpu-agent"
image = "nvidia/cuda:12.1.0-runtime-ubuntu22.04"

[container.resources]
memory = "8G"
cpu = "4"

[container.gpu]
enabled = true
type = "nvidia"

[process]
command = ["python", "train_model.py"]
```
