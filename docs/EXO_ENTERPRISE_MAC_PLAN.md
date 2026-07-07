# Exo Enterprise macOS Port Plan

## Goal

Bring **Exo Enterprise** to macOS as a Docker/Podman-competitive runtime while
preserving Exo's agent-first design:

- fast local agent/tool execution
- strong sandboxing and secret isolation
- image-based workflows
- volumes, networking, logs, exec, lifecycle
- GPU support
- daemon/API compatibility for orchestration
- a path to parity with Docker Desktop/Podman Desktop use cases

This plan treats macOS as a first-class Exo host, not a thin wrapper around
Docker, Podman, or Lima.

## Current macOS Baseline

The current macOS backend is a **native process backend**:

- runs host processes under Exo lifecycle management
- clears inherited host environment variables by default
- uses per-container Exo `HOME` and `TMPDIR`
- detects host GPU via `system_profiler`
- exposes GPU hints such as `EXO_GPU_VENDOR`, `EXO_GPU_NAME`, `EXO_GPU_METAL`
- uses `sandbox-exec` when available, with env-isolation fallback
- supports basic `run`, `run --detach`, `list`, `logs`, `stop`, `remove`, `exec`

This is useful for local agent/tool execution, but it is **not yet Docker/Podman
parity** because macOS lacks native Linux namespaces, cgroups, seccomp, overlayfs,
and Linux container networking.

## Product Definition: Exo Enterprise on macOS

Exo Enterprise on macOS should support two runtime modes:

### 1. Native Agent Mode

Purpose: fastest local execution for trusted or semi-trusted agent tools.

Characteristics:

- native macOS process execution
- strict environment isolation by default
- macOS sandbox profile when available
- per-agent home/tmp/state
- optional explicit host volume mounts
- Metal/MPS GPU access
- no Linux ABI requirement

This mode competes with lightweight local tool runners and is optimized for AI
agents, not arbitrary Linux containers.

### 2. Linux Container Mode

Purpose: Docker/Podman-compatible Linux container workflows on macOS.

Characteristics:

- OCI image execution
- Linux namespaces/cgroups/seccomp inside an Exo-managed microVM
- overlay filesystem support
- container networking and port publishing
- bind/named volumes
- logs/exec/stop/start/remove/list
- optional daemon/API surface for Claw Pen orchestration
- GPU support where possible

This mode is required to compete with Docker Desktop and Podman Desktop.

## Architectural Options

### Option A: Exo-managed microVM backend

Build and manage our own lightweight Linux VM backend for macOS.

Possible hypervisors/frameworks:

- Apple Virtualization.framework
- Hypervisor.framework
- QEMU fallback for older/non-supported systems

Pros:

- full control over UX and security model
- no dependency on Docker Desktop, Podman, or Lima
- can optimize VM image for Exo/agents
- can expose a stable Exo daemon/API
- strongest path to Docker/Podman parity

Cons:

- largest engineering lift
- requires VM image build/distribution/update story
- needs robust file sharing, networking, port forwarding, lifecycle management

Recommended long-term path.

### Option B: Integrate with an existing VM runtime as a temporary backend

Examples: Lima, Colima, vfkit, or Podman machine.

Pros:

- faster to implement Linux-container parity
- useful bridge for early enterprise users

Cons:

- not true Exo-owned experience
- dependency/version drift
- weaker product differentiation
- user explicitly prefers no Lima for core Exo

Useful only as an optional compatibility backend, not the default Exo Enterprise
macOS path.

### Option C: Native macOS-only sandboxed containers

Use only Darwin primitives: sandbox profiles, launchd/job objects, chroot-like
layouts where possible, APFS snapshots/clones, and network policy.

Pros:

- no VM overhead
- best integration with macOS
- strong for mac-native workloads

Cons:

- cannot run Linux OCI images directly
- cannot provide Docker-compatible Linux kernel semantics
- sandbox APIs are less container-oriented than Linux namespaces/cgroups

Good for Native Agent Mode, insufficient for Docker/Podman parity alone.

## Recommended Architecture

Use a **dual-backend macOS architecture**:

```text
exo CLI / Exo API
        │
        ├── native-macos backend
        │     ├── host process lifecycle
        │     ├── env/secret isolation
        │     ├── macOS sandbox profiles
        │     └── Metal/MPS GPU hints
        │
        └── macos-linux backend
              ├── Exo-managed Linux microVM
              ├── exo-runtime inside guest
              ├── OCI images/rootfs/overlay
              ├── cgroups/namespaces/seccomp
              ├── port forwarding
              ├── volume sharing
              └── daemon/API bridge
```

Runtime selection:

```bash
exo run --backend native host -- echo hello
exo run --backend linux python:3.12 python app.py
```

Default behavior:

- `host` or `--backend native` => native macOS mode
- OCI image names like `python:3.12`, `ubuntu:latest`, `ghcr.io/...` => Linux
  container mode once available
- config file can pin backend explicitly

## Phase 0: Stabilize Current Native Backend

Status: partially complete.

Tasks:

- [x] native process execution
- [x] env clearing by default
- [x] per-container home/tmp
- [x] GPU detection via `system_profiler`
- [x] basic lifecycle/logs/exec
- [ ] robust macOS sandbox profile validation on normal macOS host
- [x] add explicit `--backend native|linux|auto`
- [x] add backend field to config files
- [x] improve state-dir permissions and fallback behavior
- [x] add tests for secret isolation
- [x] add tests for volume allowlist behavior
- [x] define native mode threat model clearly

Acceptance criteria:

- `exo run --backend native host -- env` does not leak host secrets
- explicit `--env FOO=bar` passes only intended variables
- `HOME` points to Exo state, not user home
- detached lifecycle works reliably
- GPU hints are present when `--gpu` is used

## Phase 1: Backend Abstraction

Create a formal backend trait used by the CLI instead of per-command cfg blocks.

Proposed trait:

```rust
#[async_trait]
pub trait ExoBackend {
    async fn run(&self, config: ContainerConfig, opts: RunOptions) -> Result<RunResult>;
    async fn list(&self, opts: ListOptions) -> Result<Vec<ContainerMetadata>>;
    async fn start(&self, id: &str, opts: StartOptions) -> Result<()>;
    async fn stop(&self, id: &str, opts: StopOptions) -> Result<()>;
    async fn remove(&self, id: &str, opts: RemoveOptions) -> Result<()>;
    async fn logs(&self, id: &str, opts: LogOptions) -> Result<LogStream>;
    async fn exec(&self, id: &str, command: Vec<String>, opts: ExecOptions) -> Result<i32>;
    fn capabilities(&self) -> BackendCapabilities;
}
```

Capabilities should include:

- linux_containers
- native_processes
- gpu
- metal
- cgroups
- namespaces
- seccomp
- overlayfs
- port_forwarding
- volume_mounts
- daemon
- rootless

Tasks:

- [x] introduce `exo-backend` or `exo-runtime::backend` module
- [ ] migrate Linux backend behind trait
- [ ] migrate Windows WSL backend behind trait
- [ ] migrate native macOS backend behind trait
- [ ] centralize CLI dispatch and output formatting

Acceptance criteria:

- platform-specific code is mostly behind backend implementations
- commands do not duplicate Linux/Windows/macOS logic
- [x] `exo backend info` reports active backend and capabilities

## Phase 2: Exo-managed macOS Linux MicroVM MVP

Build a minimal Exo-owned Linux VM runtime for macOS.

Components:

- VM manager crate, e.g. `exo-vm-mac`
- minimal Linux image builder
- guest agent / Exo daemon inside VM
- host CLI bridge
- shared state dir
- volume sharing
- port forwarding

Recommended implementation:

- Apple Virtualization.framework for Apple Silicon/macOS 12+
- Rust bindings via existing crates or a small Objective-C/Swift helper if needed
- minimal Linux guest with:
  - exo runtime binary
  - OCI image store
  - overlayfs/fuse-overlayfs
  - cgroup v2
  - iptables/nftables or user-mode networking

Tasks:

- [ ] evaluate Rust bindings for Virtualization.framework
- [ ] prototype VM boot from minimal Linux kernel/initrd/disk
- [ ] package Exo guest image
- [ ] implement `exo vm init/start/stop/status/reset`
- [ ] install guest Exo runtime during image build, not at first run
- [ ] host-to-guest RPC over vsock/unix socket/ssh-free channel
- [ ] rootless guest runtime where possible

Acceptance criteria:

- `exo vm start` boots an Exo Linux guest
- `exo run --backend linux alpine:latest echo hi` works on macOS
- no Docker/Podman/Lima dependency
- VM starts reliably after reboot

## Phase 3: OCI Image and Filesystem Parity

Goal: Docker-compatible image workflows.

Tasks:

- [ ] `exo pull` inside guest for linux/arm64 and linux/amd64 images
- [ ] platform selection: `--platform linux/arm64`, `linux/amd64`
- [ ] content-addressable layer store
- [ ] overlayfs/fuse-overlayfs support
- [ ] named volumes
- [ ] bind mounts from macOS host into guest
- [ ] image import/export
- [ ] image prune
- [ ] rootfs persistence across restart

macOS-specific requirements:

- file sharing must preserve enough permissions for agent workloads
- support project directory mounts under `/Users/...`
- avoid exposing entire user home by default
- require explicit volume declarations for host data

Acceptance criteria:

- `exo run ubuntu:latest uname -a`
- `exo run -v $PWD:/app python:3.12 python /app/script.py`
- `exo pull`, `exo images`, `exo import` work on macOS Linux backend

## Phase 4: Networking and Ports

Goal: compete with Docker Desktop local dev workflows.

Tasks:

- [ ] bridge/user-mode network inside guest
- [ ] DNS resolution inside containers
- [ ] `-p host:container` port forwarding
- [ ] IPv4 localhost forwarding to macOS
- [ ] container-to-container networking
- [ ] network modes: bridge, host-like, none
- [ ] firewall safety rules

Acceptance criteria:

- `exo run -p 8080:80 nginx` reachable at `localhost:8080`
- multiple port mappings work
- stopped/removed containers release ports

## Phase 5: Security and Secrets

Goal: stronger than Docker defaults for AI agents.

Tasks:

- [ ] secret store abstraction
- [ ] `exo secret set/get/list/remove`
- [ ] explicit secret injection only: env/file mount/session token
- [ ] never inherit host env by default on any backend
- [ ] per-agent policy profile
- [ ] denylist/allowlist for host path mounts
- [ ] audit log of secret access and volume mounts
- [ ] macOS Keychain integration for host-side encrypted secret storage
- [ ] guest-side tmpfs secret mounts

Acceptance criteria:

- agents cannot see host secrets unless explicitly granted
- secret access is auditable
- secrets do not persist in logs or metadata by default

## Phase 6: GPU Support

### Native macOS backend

Status: implemented as host GPU exposure/hints.

Tasks:

- [x] detect Apple/AMD/NVIDIA/Intel GPUs via `system_profiler`
- [x] expose Metal/MPS environment hints
- [ ] add `exo gpu list --json`
- [ ] add GPU capability reporting in backend info
- [ ] test PyTorch MPS and MLX workloads

### Linux microVM backend

GPU passthrough for Linux containers on macOS is harder.

Possible approaches:

- native host mode for Metal/MPS workloads
- guest Linux CPU fallback
- future Apple Virtualization GPU/virtio-gpu capabilities if sufficient
- specialized acceleration bridge for agent ML workloads

Tasks:

- [ ] document limitations honestly
- [ ] benchmark native MPS vs Linux guest CPU
- [ ] support backend routing: GPU requested on macOS defaults to native unless
      image requires Linux
- [ ] consider MLX/PyTorch-MPS optimized base workflows

Acceptance criteria:

- `exo run --gpu host -- python mps_test.py` works natively
- Linux backend provides clear error or fallback when GPU cannot be passed through

## Phase 7: Daemon/API and Claw Pen Integration

Goal: stable control plane for Claw Pen orchestration.

Tasks:

- [ ] daemon API over Unix socket on macOS host
- [ ] backend-aware run/list/stop/logs endpoints
- [ ] event stream
- [ ] health/status API
- [ ] structured JSON output for all commands
- [ ] Claw Pen config option to choose Exo backend
- [ ] migration path from Docker client to Exo client

Acceptance criteria:

- Claw Pen can create, start, stop, list, and inspect agents through Exo
- logs stream reliably
- Exo backend failures surface actionable errors

## Phase 8: Docker/Podman Compatibility Layer

Goal: reduce migration friction.

Tasks:

- [ ] CLI aliases / compatible flags for common commands
- [ ] compose-like subset or adapter
- [ ] Dockerfile/build support or integration with buildkit-compatible builder
- [ ] container inspect JSON compatibility subset
- [ ] image/tag naming compatibility
- [ ] registry auth support

Acceptance criteria:

- common Docker workflows map cleanly to Exo equivalents
- docs include Docker/Podman migration guide

## Phase 9: Packaging and Enterprise Distribution

Tasks:

- [ ] signed macOS universal binary
- [ ] Homebrew tap
- [ ] pkg installer
- [ ] auto-update or managed update channel
- [ ] notarization
- [ ] enterprise config profiles
- [ ] uninstall/reset tooling
- [ ] versioned guest VM image updates

Acceptance criteria:

- clean install on fresh macOS host
- Claw Pen can depend on `exo` being available
- enterprise admins can pin versions and configure policies

## Phase 10: Observability and Reliability

Tasks:

- [ ] structured logs
- [ ] event DB cleanup/rotation
- [ ] runtime diagnostics bundle
- [ ] `exo doctor`
- [ ] VM health checks
- [ ] crash recovery and orphan cleanup
- [ ] performance benchmarks vs Docker Desktop and Podman Desktop

Acceptance criteria:

- support can diagnose failures quickly
- Exo survives host reboot and stale process/VM state
- performance targets are tracked continuously

## Proposed Milestones

### M0 — Native macOS Developer Preview

- native mode stable
- secret isolation default
- GPU hints
- docs
- Claw Pen can run local host-mode agents

### M1 — Backend trait and capability model

- all platform backends behind one abstraction
- backend selection and `exo backend info`

### M2 — Exo Linux microVM MVP

- boot Exo-owned Linux guest
- run simple OCI container
- no Docker/Podman/Lima dependency

### M3 — Docker Desktop parity core

- pull/run/list/logs/exec/stop/remove
- volumes
- port publishing
- image cache

### M4 — Enterprise security

- secrets
- policies
- audit logs
- macOS Keychain integration
- signed packages

### M5 — Claw Pen production readiness

- daemon/API stable
- end-to-end Claw Pen integration
- diagnostics
- upgrade path
- documented support matrix

## Open Questions

- Should Linux-container mode be mandatory for all non-`host` images, or should
  native mode support named host images as labels only?
- What is the minimum macOS version to support?
- Should Exo provide a Docker socket compatibility mode, or only an Exo-native API?
- How much Compose compatibility is required for Claw Pen Enterprise?
- What GPU workloads matter most: MLX, PyTorch MPS, Core ML, llama.cpp Metal,
  or Linux CUDA/ROCm compatibility?
- What is the desired enterprise secret-store backend: macOS Keychain only, or
  pluggable providers?

## Immediate Next Steps

1. [x] Add `--backend native|linux|auto` to CLI/config.
2. [x] Add `exo backend info` and `exo gpu list`.
3. [ ] Formalize backend trait and move current Linux/native macOS paths behind it.
4. Validate macOS sandbox behavior on a normal non-managed host.
5. Build the first Apple Virtualization.framework VM boot prototype.
6. Define Claw Pen orchestrator integration tests against installed `exo`.
