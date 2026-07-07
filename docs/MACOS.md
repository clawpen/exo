# macOS Support

Exo now has a **native macOS backend**. It does not use Lima, Docker Desktop, or
WSL. It runs agent/tool workloads as macOS host processes while preserving Exo's
CLI shape, lifecycle metadata, logs, and basic start/stop/list/remove semantics.

## Why native process mode?

The Linux Exo backend uses Linux-only primitives:

- namespaces
- cgroups
- seccomp
- overlayfs
- `/proc`-based process inspection

Those primitives do not exist on Darwin. Rather than hiding that behind a VM,
the macOS backend is explicit: it is a native process runtime for Exo workloads
that need portable Rust orchestration and fast local execution, not Linux kernel
container isolation.

## Usage

```bash
# Run a host command through Exo's macOS backend
exo run host -- echo "hello from native macOS Exo"

# Make the backend choice explicit
exo run --backend native host -- echo "hello from native macOS Exo"

# Run in the background with Exo lifecycle metadata
exo run --name sleeper --detach host -- sleep 3600

# Inspect and manage it
exo list --all
exo logs sleeper
exo stop sleeper
exo remove sleeper
```

The image argument is retained for CLI compatibility and metadata. In native
macOS mode it is a label; the command runs on the host.

Backend selection:

- `--backend native` pins native host-process mode.
- `--backend linux` is reserved for the future Exo-managed Linux microVM backend
  and currently returns an actionable error on macOS.
- `--backend auto` is the default; until the Linux microVM backend lands, macOS
  auto mode runs through the native backend and warns for non-`host` images.

Diagnostics:

```bash
exo backend info
exo backend info --json
exo gpu list
exo gpu list --json
exo doctor
exo doctor --json
```

## Secrets

Native mode does not inherit host environment variables. Store secrets
explicitly and inject them by name:

```bash
exo secret set OPENAI_API_KEY --value "$OPENAI_API_KEY"
exo secret list
exo run --backend native --secret OPENAI_API_KEY host -- env
```

Only secret names are recorded in container configuration; secret values are
loaded from the local Exo secret store at process spawn time.

## Named volumes

Exo supports named volumes:

```bash
exo volume create data
exo volume list
exo volume inspect data
exo run --backend native -v data:/data --workdir /data host -- sh -c 'echo hello > file.txt'
exo volume remove data
```

In native macOS mode, volumes are backed by Exo-managed host directories. Since
native mode is not a chroot or VM, it cannot create a real absolute `/data`
mount in the host root. Instead, Exo resolves `--workdir /data` to the backing
volume path and allows/sandboxes the volume directory. True Linux-style mount
semantics for arbitrary container paths are part of the `--backend linux`
microVM backend.

## Sandbox policy

Native mode defaults to best-effort sandboxing:

```bash
exo run --backend native --sandbox auto host -- echo hello
```

Use `--sandbox required` to fail closed if macOS `sandbox-exec` is unavailable
or blocked by the host:

```bash
exo run --backend native --sandbox required host -- ./trusted-tool
```

Use `--sandbox off` only for debugging trusted workloads.

## Supported commands

- `run`
- `list` / `ps`
- `start`
- `stop`
- `remove` / `rm`
- `logs`
- `exec` (runs another host command using the saved container context)
- `pull`, `images`, `import` via the existing Rust image store
- `events` where the shared event log is available

## Current limitations

The native backend does **not** enforce Linux container isolation. It does, however,
protect secrets by default in two ways:

- host environment variables are **not inherited**; only explicit `--env KEY=VALUE`
  values are passed through
- each run gets an Exo-owned `HOME` and `TMPDIR`, not your real home directory
- when `sandbox-exec` is available, Exo writes a per-container macOS sandbox
  profile that allows system reads plus Exo state / explicit volume paths and
  denies broad file access

Set `EXO_MAC_SANDBOX=0` only for debugging if the macOS system sandbox blocks a
workload you trust. In restricted host environments where `sandbox-exec` itself
is not permitted, Exo falls back to env isolation and logs a warning.

These Linux-specific options are accepted for compatibility but are not enforced
with Linux kernel semantics on macOS:

- cgroup resource limits (`--memory`, `--cpu`, pid limits)
- Linux namespaces / network isolation
- seccomp profiles
- readonly rootfs / pivot-root / overlayfs
- user switching via `exec --user`

For strict Linux container isolation, use Exo on Linux. For portable local agent
execution on macOS, use this native backend.

See [`MACOS_NATIVE_THREAT_MODEL.md`](MACOS_NATIVE_THREAT_MODEL.md) for the
explicit native-mode threat model.

## GPU support

`--gpu` is supported on macOS native mode as host GPU exposure. Since the
workload is already a macOS host process, no device node needs to be mounted.
Exo detects GPUs with `system_profiler SPDisplaysDataType -json` and injects
metadata/environment hints such as:

- `EXO_GPU=1`
- `EXO_GPU_VENDOR=apple|amd|nvidia|intel|unknown`
- `EXO_GPU_NAME=...`
- `EXO_GPU_METAL=1` when Metal is available
- `EXO_GPU_CORES=...` for Apple Silicon when reported
- `EXO_GPU_VRAM_MB=...` for discrete GPUs when reported

For Apple Silicon Metal/MPS workloads, Exo also sets
`PYTORCH_ENABLE_MPS_FALLBACK=1` unless the user overrides it explicitly.

Example:

```bash
exo run --gpu host -- sh -c 'echo $EXO_GPU_VENDOR $EXO_GPU_NAME $EXO_GPU_METAL'
```
