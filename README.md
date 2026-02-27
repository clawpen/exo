# Exo - Agent Container Runtime

> **Exō** (Latin): "from outside, outward" — the protective shell that makes agents possible.

A container runtime built specifically for AI agents from [Claw Pen](https://clawpen.ca).

## Why Exo?

General-purpose container runtimes (Docker, containerd) were designed for microservices. Exo is designed for **agents**:

- **Agent-first communication** — Stdio + tool bus, not HTTP
- **Tool-level sandboxing** — Each tool gets its own security context
- **Fast spawning** — Daemonless, spin up in milliseconds
- **Rootless by default** — User namespaces, no system privileges required

## Quick Start

```bash
# Run a Python agent container
exo run --image python:3.12 --tool bash

# Run with GPU support
exo run --gpu --image python:3.12

# List running containers
exo ps

# Stop a container
exo stop <container-id>
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      Exo CLI                              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │                    Agent Channel                       │  │
│  │  • Stdio-based messaging (not HTTP)                 │  │
│  │  • Tool bus for sandboxed command execution        │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │                  Storage Layer                         │  │
│  │  • Overlay2 filesystem                               │  │
│  │  • Layer management                                  │  │
│  │  • Image operations                                  │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │                  Security Layer                        │  │
│  │  • Linux namespaces (user, pid, net, mount)         │  │
│  │  • Cgroups v2 (memory, CPU, I/O limits)            │  │
│  │  • Seccomp syscall filtering                         │  │
│  │  • Capability dropping                               │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    Container                               │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐                        │
│  │  bash   │  │  python  │  │  node   │  Agent Tools           │
│  └─────────┘  └─────────┘  └─────────┘                        │
└─────────────────────────────────────────────────────────────┘
```

## Project Structure

```
exo/
├── Cargo.toml                    # Workspace
├── crates/
│   ├── exo/                      # CLI application
│   │   └── src/commands/         # run, ps, stop, logs, images
│   ├── exo-runtime/             # Core container runtime
│   │   ├── src/
│   │   │   ├── container.rs      # Container lifecycle
│   │   │   ├── config.rs         # Configuration
│   │   │   ├── namespace.rs     # Linux namespaces
│   │   │   ├── userns.rs         # User namespaces
│   │   │   ├── rootfs.rs        # Root filesystem
│   │   │   ├── cgroup.rs         # Cgroup v2
│   │   │   ├── security.rs       # Capabilities
│   │   │   ├── seccomp.rs       # Syscall filtering
│   │   │   ├── binfmt.rs        # Foreign binary support
│   │   │   ├── storage.rs       # Overlay2 storage
│   │   │   ├── image.rs         # Image management
│   │   │   ├── channel.rs       # Agent communication
│   │   │   └── process.rs       # Process spawning
│   ├── exo-image/               # OCI image operations
│   ├── exo-wsl/                 # WSL2 backend (Windows)
│   └── exo-gpu/                 # GPU detection
└── docs/                        # Architecture & design docs
```

## Agent Communication Protocol

Exo speaks "Agent" natively:

```
Host → Agent: {"type": "tool_request", "tool": "bash", "args": {...}}
Agent → Host: {"type": "observation", "content": "result..."}

Tool Bus:
┌─────────────┐     ┌─────────────┐
│   Agent     │────▶│  Exo        │────▶│   Bash      │
│  (stdin)    │     │ (Tool Bus)  │     │ (isolated)  │
└─────────────┘     └─────────────┘     └─────────────┘
```

No HTTP overhead, no WebSocket gymnastics. Just structured stdio.

## Windows Support via WSL2

On Windows, Exo uses WSL2 as a Linux backend:

```
Windows ──────▶ WSL2 ──────▶ Linux Container
                   ↓
              GPU Passthrough
              Filesystem Mount
```

Single `exo.exe` handles WSL2 installation, distro management, and container execution.

## For All Agents

Exo is designed to serve any AI agent that needs sandboxed tool execution:

- **OpenClaw** - The LLM that started it all
- **Agent-0** - Coding agents
- **Your agents** - Whatever you're building

One container runtime, many agents.

## Status

- [x] Container isolation (namespaces, cgroups, seccomp)
- [x] Storage layer (overlay2, OCI images)
- [x] Agent communication protocol
- [x] Windows support (WSL2 backend)
- [x] GPU passthrough
- [ ] Container networking (bridge mode)
- [ ] Multi-agent orchestration
- [ ] Kubernetes integration

## License

MIT OR Apache-2.0

---

**Exo** — *The outer shell that protects your agents.*
From [Claw Pen](https://clawpen.ca)
