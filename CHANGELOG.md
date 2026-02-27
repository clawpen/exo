# Changelog

All notable changes to Exo will be documented in this file.

## [1.0.0-alpha] - 2025-02-27

### Added
- Complete Linux container runtime with full isolation:
  - User namespaces for rootless operation
  - PID, Mount, UTS, IPC, Network, Cgroup namespaces
  - Cgroup v2 resource limiting (memory, CPU, PIDs, I/O)
  - Linux capabilities management
  - Seccomp syscall filtering with multiple profiles
  - pivot_root(2) for filesystem isolation
- WSL2 backend for Windows compatibility
- Overlay2 storage driver for layered filesystems
- OCI image management and parsing
- Foreign binary support (ARM on x86 via QEMU binfmt)
- Agent channel protocol (stdio-based)
- CLI: `exo run`, `exo build`, `exo pull`, `exo push`

### Status
This is an **alpha release**. The architecture is complete but:
- Untested on production Linux systems
- Image pulling needs real-world testing
- E2E integration tests not yet verified
- Agent protocol integration incomplete
- Container state management is basic

Exo is ready for experimentation and feedback.

### Philosophy
Exo is a container runtime **for AI agents**, not microservices.
- Agent-first: stdio + tool bus, not HTTP
- Daemonless: fast spawning, no background services
- Rootless: user namespaces by default
- Tool-level sandboxing: per-tool security contexts

[1.0.0-alpha]: https://github.com/clawpen/exo/releases/tag/v1.0.0-alpha
