# Exo CLI Agent Reference

> **GENERATED — do not edit by hand.** Regenerate with `exo agent-docs > docs/AGENT_CLI.md`. A contract test fails the build when this file is stale.
>
> Source of truth: the clap definitions in `crates/exo/src/main.rs`. The error contract (exit codes, error envelope) lives in `docs/EXIT_CODES.md`.

The primary consumer of this CLI is an AI agent. Everything below is stable within schema 1: commands, flags, JSON payload shapes, exit codes. Additive changes only.

## Global flags

Available on every command (place before the container's argv in `run`/`exec`):

| Flag | Description |
|---|---|
| `--debug` | Enable debug logging |
| `-q, --quiet` | Quiet mode (minimal output) |
| `--json` | Output machine-readable JSON (agent contract, schema 1). On failure, a structured error envelope is printed to stderr and the process exits with the documented code (docs/EXIT_CODES.md) |

## Commands

### `exo run`

Run a container

**Usage:** `exo run [OPTIONS] <image> [command...]`

| Argument | Required | Description |
|---|---|---|
| `<image>` | yes | Container image (e.g., python:3.12) |
| `[command...]` | no | Command to run (follows `--`) |

| Flag | Value | Default | Description |
|---|---|---|---|
| `-n, --name` | NAME | — | Container name |
| `-c, --config` | FILE | — | Config file (TOML) |
| `--workdir` | DIR | — | Working directory |
| `--workspace` | DIR | — | Host workspace directory to stream into the container and pull back after the run (macOS Linux microVM backend only) |
| `-v, --volume` | SRC:DEST (repeatable) | — | Volume mounts (source:target) |
| `-e, --env` | KEY=VALUE (repeatable) | — | Environment variables (KEY=VALUE) |
| `--secret` | NAME (repeatable) | — | Secret names to inject from `exo secret` as environment variables |
| `--gpu` | — | — | Enable GPU passthrough |
| `--gpu-type` | TYPE | — | GPU type (nvidia, amd, auto) |
| `-m, --memory` | LIMIT | — | Memory limit (e.g., 2G, 512M) |
| `--cpu` | LIMIT | — | CPU limit (e.g., 2, 200%) |
| `--network` | MODE | — | Network mode (bridge, host, none) |
| `-p, --publish` | HOST:CONT (repeatable) | — | Port mappings (host:container) |
| `--rm` | — | — | Remove container on exit |
| `-i, --interactive` | — | — | Interactive mode (keep STDIN open) |
| `-t, --tty` | — | — | Allocate a pseudo-TTY |
| `-d, --detach` | — | — | Detach from container (run in background) |
| `--backend` | BACKEND | auto | Runtime backend: auto, native, or linux |
| `--sandbox` | MODE | auto | Host sandbox mode: auto, off, or required |

### `exo list`

List running containers

Aliases: `ps`

**Usage:** `exo list [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `-a, --all` | — | — | Show all containers (including stopped) |
| `--backend` | BACKEND | auto | Runtime backend: auto, native, or linux |

### `exo start`

Start a stopped container

**Usage:** `exo start [OPTIONS] <container>`

| Argument | Required | Description |
|---|---|---|
| `<container>` | yes | Container ID or name |

| Flag | Value | Default | Description |
|---|---|---|---|
| `-a, --attach` | — | — | Attach to container output |
| `--backend` | BACKEND | auto | Runtime backend: auto, native, or linux |

### `exo stop`

Stop a running container

**Usage:** `exo stop [OPTIONS] <container>`

| Argument | Required | Description |
|---|---|---|
| `<container>` | yes | Container ID or name |

| Flag | Value | Default | Description |
|---|---|---|---|
| `-f, --force` | — | — | Force stop (SIGKILL) |
| `-t, --time` | TIME | 10 | Wait time before force killing (seconds) |
| `--backend` | BACKEND | auto | Runtime backend: auto, native, or linux |

### `exo remove`

Remove a container

Aliases: `rm`

**Usage:** `exo remove [OPTIONS] <container>`

| Argument | Required | Description |
|---|---|---|
| `<container>` | yes | Container ID or name |

| Flag | Value | Default | Description |
|---|---|---|---|
| `-f, --force` | — | — | Force remove (even if running) |
| `--backend` | BACKEND | auto | Runtime backend: auto, native, or linux |

### `exo logs`

View container logs

**Usage:** `exo logs [OPTIONS] <container>`

| Argument | Required | Description |
|---|---|---|
| `<container>` | yes | Container ID or name |

| Flag | Value | Default | Description |
|---|---|---|---|
| `-f, --follow` | — | — | Follow log output |
| `-t, --tail` | TAIL | 100 | Show last N lines |
| `--timestamps` | — | — | Show timestamps |
| `--backend` | BACKEND | auto | Runtime backend: auto, native, or linux |

### `exo exec`

Execute a command in a running container

**Usage:** `exo exec [OPTIONS] <container> [command...]`

| Argument | Required | Description |
|---|---|---|
| `<container>` | yes | Container ID or name |
| `[command...]` | no | Command to execute |

| Flag | Value | Default | Description |
|---|---|---|---|
| `-i, --interactive` | — | — | Interactive mode |
| `-t, --tty` | — | — | Allocate a pseudo-TTY |
| `--user` | USER | — | User to run as |
| `--backend` | BACKEND | auto | Runtime backend: auto, native, or linux |

### `exo pull`

Pull an image

**Usage:** `exo pull <image>`

| Argument | Required | Description |
|---|---|---|
| `<image>` | yes | Image to pull |

### `exo images`

List images

**Usage:** `exo images [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `-a, --all` | — | — | Show all images (including intermediate) |

### `exo import`

Import image from tarball

**Usage:** `exo import [OPTIONS] <tarball>`

| Argument | Required | Description |
|---|---|---|
| `<tarball>` | yes | Path to image tarball |

| Flag | Value | Default | Description |
|---|---|---|---|
| `-n, --name` | NAME | — | Name for imported image (e.g., myimage:latest) |

### `exo secret`

Manage local Exo secrets

**Usage:** `exo secret <SUBCOMMAND>`

#### `exo secret set`

Store a secret value (from --value, matching env var, or stdin)

**Usage:** `exo secret set [OPTIONS] <name>`

| Argument | Required | Description |
|---|---|---|
| `<name>` | yes | Secret name |

| Flag | Value | Default | Description |
|---|---|---|---|
| `--value` | VALUE | — | Secret value; omit to read env var with the same name or stdin |

#### `exo secret list`

List secret names (values are never printed)

**Usage:** `exo secret list`

#### `exo secret remove`

Remove a secret

**Usage:** `exo secret remove <name>`

| Argument | Required | Description |
|---|---|---|
| `<name>` | yes | Secret name |

### `exo doctor`

Diagnose host readiness for Exo

**Usage:** `exo doctor`

### `exo events`

Show the daemon's lifecycle event log

**Usage:** `exo events [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `-c, --container` | CONTAINER | — | Filter to one container (by id or name) |
| `-l, --limit` | LIMIT | 50 | Maximum events to show (newest first) |

### `exo daemon`

Daemon mode - run a persistent server for faster operations

**Usage:** `exo daemon [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `--foreground` | — | — | Run in foreground (don't detach) |
| `--stop` | — | — | Stop the daemon |
| `--status` | — | — | Show daemon status |
| `--socket` | PATH | — | Socket path (default: /tmp/exo-daemon.sock) |
| `--timeout` | TIMEOUT | 30000 | Request timeout in milliseconds |

### `exo backend`

Show backend information and capabilities

**Usage:** `exo backend <SUBCOMMAND>`

#### `exo backend info`

Show active backend and capabilities

**Usage:** `exo backend info`

### `exo gpu`

Inspect GPU availability

**Usage:** `exo gpu <SUBCOMMAND>`

#### `exo gpu list`

List detected GPUs

**Usage:** `exo gpu list`

### `exo vm`

Manage the Exo Linux microVM (macOS only)

Aliases: `machine`

**Usage:** `exo vm <SUBCOMMAND>`

#### `exo vm init`

Download/build the guest image

**Usage:** `exo vm init [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `--force` | — | — | Force re-download and rebuild |

#### `exo vm start`

Start the VM

**Usage:** `exo vm start [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `--foreground` | — | — | Run in foreground and attach to VM output |

#### `exo vm stop`

Stop the VM

**Usage:** `exo vm stop [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `-f, --force` | — | — | Force stop |

#### `exo vm status`

Show VM status

**Usage:** `exo vm status`

#### `exo vm reset`

Reset the VM image and state

**Usage:** `exo vm reset [OPTIONS]`

| Flag | Value | Default | Description |
|---|---|---|---|
| `--keep-state` | — | — | Keep runtime state file |

#### `exo vm install-guest-agent`

Install a built guest agent binary for embedding during `exo vm init`

**Usage:** `exo vm install-guest-agent <path>`

| Argument | Required | Description |
|---|---|---|
| `<path>` | yes | Path to exo-vm-guest-init built for the Linux guest architecture |

#### `exo vm import-image`

Import an image rootfs tarball already visible inside the guest VM

**Usage:** `exo vm import-image [OPTIONS] <image>`

| Argument | Required | Description |
|---|---|---|
| `<image>` | yes | Image name/tag to register in guest state |

| Flag | Value | Default | Description |
|---|---|---|---|
| `--guest-path` | PATH | — | Path to a tar or tar.gz archive inside the guest VM |

#### `exo vm rm-image`

Remove an image rootfs from the guest store

**Usage:** `exo vm rm-image <image>`

| Argument | Required | Description |
|---|---|---|
| `<image>` | yes | Image name/tag to remove from guest state |

### `exo volume`

Manage named volumes

**Usage:** `exo volume <SUBCOMMAND>`

#### `exo volume create`

Create a named volume

**Usage:** `exo volume create <name>`

| Argument | Required | Description |
|---|---|---|
| `<name>` | yes | Volume name |

#### `exo volume list`

List named volumes

Aliases: `ls`

**Usage:** `exo volume list`

#### `exo volume inspect`

Inspect a named volume

**Usage:** `exo volume inspect <name>`

| Argument | Required | Description |
|---|---|---|
| `<name>` | yes | Volume name |

#### `exo volume remove`

Remove a named volume

Aliases: `rm`

**Usage:** `exo volume remove <name>`

| Argument | Required | Description |
|---|---|---|
| `<name>` | yes | Volume name |

## JSON success payloads (schema 1)

With `--json`, every success payload carries `"schema": 1` on stdout. Shapes per command:

| Command | Payload |
|---|---|
| `run -d` | `{"schema":1,"id","name","detached":true}` |
| `run (attach)` | `container output streams raw; exit code carries the result (`CONTAINER_EXITED`)` |
| `stop` | `{"schema":1,"container","status":"stopped"\|"not_running"}` |
| `start` | `{"schema":1,"container","status":"started"\|"already_running"}` |
| `rm` | `{"schema":1,"container","status":"removed"}` |
| `exec` | `{"schema":1,"container","exit_code"}` |
| `logs` | `{"schema":1,"container","content"}` |
| `pull` | `{"schema":1,"image","cached","config_digest","layers"}` |
| `images` | `{"schema":1,"images":[{"repository","tag","registry"}…]}` |
| `list / ps` | `JSON array of container objects` |
| `doctor / events / backend info / gpu list / vm status / secret list / volume ls / volume inspect` | `per-command objects` |

Lifecycle `status` strings are additive-only: `stopped`, `not_running`, `started`, `already_running`, `removed`.

## Errors

Failures emit a structured envelope on **stderr** and exit with a documented class code (never 1):

```json
{"schema":1,"error":{"code":"CONTAINER_NOT_FOUND","message":"container not found: web","retryable":false}}
```

| Exit | Class |
|---|---|
| 0 | success |
| 2 | not found |
| 3 | conflict / state |
| 4 | backend / registry |
| 5 | invalid input |
| 6 | internal |

Exception: attach-mode `run`/`exec`/`start --attach` exit with the **container's own** exit code; the envelope's `CONTAINER_EXITED` code disambiguates a workload exit from an exo failure.

The full taxonomy, `code` strings, `retryable` semantics, and the idempotency matrix are in `docs/EXIT_CODES.md`.
