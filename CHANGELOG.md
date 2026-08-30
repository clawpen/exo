# Changelog

All notable changes to Exo will be documented in this file.

## [Unreleased]

### Added
- **Idempotent lifecycle verbs (A5):** desired-state semantics documented in
  `docs/EXIT_CODES.md` — `stop` on a stopped container succeeds
  (`not_running`), `start` on a running one succeeds (`already_running`),
  absent targets always exit 2 `CONTAINER_NOT_FOUND`, `rm` on running without
  `--force` exits 3 `CONTAINER_RUNNING`.
- `exo-vm-mac` maps stringly guest-RPC errors onto the typed taxonomy
  (transitional `map_guest_error`); VM "not ready" is now typed
  `BACKEND_UNAVAILABLE` (retryable); follow-logs and interactive exec raise
  typed `BACKEND_UNSUPPORTED`.
- `crates/exo/tests/cli_contract.rs`: integration tests driving the real
  binary — exit-code classes, stderr error envelope, schema-1 payloads.

- **Agent error contract (A1/A2):** typed `ExoError` taxonomy in `exo-runtime`
  with stable machine-readable codes (`CONTAINER_NOT_FOUND`, `BACKEND_UNSUPPORTED`,
  …), documented exit-code classes (2 not-found, 3 conflict, 4 backend,
  5 invalid-input, 6 internal — never 1), a `retryable` flag, and a
  schema-1 JSON error envelope for `--json` output. Contract documented in
  `docs/EXIT_CODES.md`.
- `exo-runtime::exit_code_for` recovers typed codes through `anyhow` chains,
  so converted and legacy code paths share one process boundary.
- Roadmap rewritten from a code-level survey: parity matrix, defect list
  (D1–D8), three pillars (agent contract / stability / backend completion).

### Changed
- **`--json` everywhere (A3/A4):** `--json` is now a global flag (per-command
  dupes removed). Lifecycle commands emit schema-1 JSON payloads
  (`run -d` → `{id,name,detached}`, `stop`/`start`/`rm` → `{container,status}`,
  `exec` → `{exit_code}`, `logs` → `{content}`, `pull`/`images` → object
  shapes in `docs/EXIT_CODES.md`). Failures in `--json` mode emit the
  structured error envelope on **stderr** (stdout stays pure data); log noise
  is suppressed to errors unless `--debug`.
- `exo-mac` raises typed `CONTAINER_NOT_FOUND` / `SECRET_NOT_FOUND`;
  `exo-vm-mac` raises typed `BACKEND_UNSUPPORTED` for resource limits, GPU,
  and host bind mounts.
- CLI `main` returns typed `ExitCode`; core lifecycle commands (`run`, `stop`,
  `start`, `rm`, `exec`, `logs`, `pull`, `import`, backend selection) raise
  typed errors at their boundaries.
- Linux `exec`/`logs` placeholders no longer fake success — they fail with
  `BACKEND_UNSUPPORTED` (exit 4) until the real implementations land (D1).

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
