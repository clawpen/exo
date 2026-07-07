# macOS Native Backend Threat Model

This document scopes the security guarantees for Exo's macOS **native agent
mode**. Native mode is a host-process runner for local agent/tool workloads; it
is not a Linux container runtime.

## Assets protected by default

- Host environment secrets such as API keys, tokens, shell exports, and
  inherited credential paths.
- The user's real home directory as the process `HOME`.
- The user's real temp directory as the process `TMPDIR`.
- Host filesystem paths that are not part of Exo state or explicitly mounted,
  when `sandbox-exec` is available on the host.

## Trust boundary

Native mode starts a regular macOS child process owned by the current user. Exo
controls the child's lifecycle metadata, stdio/log files, environment, working
directory, and optional macOS sandbox profile. The process is still a Darwin
process and shares the host kernel, user identity, network stack, and hardware.

Use native mode for trusted or semi-trusted local agent tools where fast startup,
Metal/MPS access, and secret isolation are more important than Linux ABI or
kernel-level container isolation.

## Default protections

- `env_clear()` is used before spawning the workload.
- Only Exo's safe baseline environment and explicit `--env KEY=VALUE` entries
  are passed through.
- `HOME`, `TMPDIR`, `TMP`, and `TEMP` point at Exo-owned per-container
  directories. User overrides of those keys are ignored.
- `EXO_BACKEND=native-macos` is injected for diagnostics.
- `sandbox-exec` is used when available and preflight validation succeeds.
- The generated sandbox profile denies by default, allows system reads, Exo
  state, the resolved workdir, log files, and explicit volume sources.

## Non-goals

Native mode does **not** provide:

- Linux namespaces.
- Linux cgroups.
- Linux seccomp.
- Overlayfs/pivot-root semantics.
- A separate Linux network namespace.
- Strong isolation from same-user macOS processes if `sandbox-exec` is
  unavailable or disabled.

For strict Linux container isolation on macOS, use the future Exo-managed Linux
microVM backend. Until then, use Exo on Linux for Linux container semantics.

## Operational guidance

- Prefer `exo run --backend native host -- ...` for native macOS agents.
- Use explicit `--env` or future Exo secrets commands instead of relying on
  inherited shell secrets.
- Mount host paths explicitly with `-v SRC:DEST`; avoid mounting the full home
  directory.
- Keep `EXO_MAC_SANDBOX` enabled unless debugging a trusted workload.
- In restricted environments where state cannot be created in the default
  location, set `EXO_STATE_DIR` to a persistent writable directory.
