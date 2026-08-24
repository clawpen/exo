# Exo Exit Codes & Error Contract

> **Status:** contract, schema 1 — additive-only. Codes and exit codes documented
> here are stable; agents driving exo may branch on them. Never rename or
> renumber without a schema-version bump.
>
> Implementation: `crates/exo-runtime/src/error.rs` (`ExoError`).

## Why

The primary consumer of the exo CLI is an AI agent (via Orchestre), not a
human. Agents must not parse prose to decide what to do next. Every failure
therefore carries three machine-readable facts:

1. an **exit code** (process boundary),
2. a stable **`code` string** (JSON boundary, see `--json` below),
3. a **`retryable` flag** — whether retrying the same request as-is may succeed.

## Exit-code taxonomy

| Code | Class | Meaning | Agent action |
|---|---|---|---|
| 0 | OK | success | — |
| 2 | not found | container/image/volume/secret does not exist | create it, or fix the reference |
| 3 | conflict | request conflicts with current state (already exists / running / stopped) | inspect state, then retry differently |
| 4 | backend | backend unavailable, daemon unreachable, or feature unsupported by this backend | check `exo doctor`, retry if `retryable`, or pick another backend |
| 5 | invalid input | malformed flags, names, references, missing files | fix the request; never retry as-is |
| 6 | internal | bugs, I/O failures, unconverted legacy errors | report; retrying rarely helps |

Exit code 1 is **never** used for failures.

## Error codes (JSON `code` strings)

| `code` | Exit | Retryable | Message shape |
|---|---|---|---|
| `CONTAINER_NOT_FOUND` | 2 | no | `container not found: <id>` |
| `IMAGE_NOT_FOUND` | 2 | no | `image not found: <ref>` |
| `VOLUME_NOT_FOUND` | 2 | no | `volume not found: <name>` |
| `SECRET_NOT_FOUND` | 2 | no | `secret not found: <name>` |
| `CONTAINER_ALREADY_EXISTS` | 3 | no | `container already exists: <name>` |
| `CONTAINER_RUNNING` | 3 | no | `container is running: <id> (<hint>)` |
| `CONTAINER_NOT_RUNNING` | 3 | no | `container is not running: <id>` |
| `DAEMON_UNREACHABLE` | 4 | yes | `daemon unreachable: <detail>` |
| `BACKEND_UNAVAILABLE` | 4 | yes | `backend unavailable: <detail>` |
| `BACKEND_UNSUPPORTED` | 4 | no | `feature '<f>' is not supported by the '<b>' backend` |
| `INVALID_INPUT` | 5 | no | `invalid input: <detail>` |
| `INVALID_NAME` | 5 | no | `invalid name: <detail>` |
| `INTERNAL` | 6 | no | `internal error: <detail>` |
| `IO` | 6 | yes | `I/O error: <detail>` |

## JSON error envelope

`--json` is a **global** flag: `exo --json <command> …` or `exo <command> … --json`
(but place it *before* the container command in `exo run`/`exo exec` — anything
after the image is captured as the container's argv). On failures, the
structured envelope is emitted on **stderr** (stdout stays pure data), and the
process exits with the documented code:

```json
{
  "schema": 1,
  "error": {
    "code": "CONTAINER_NOT_FOUND",
    "message": "container not found: web",
    "retryable": false
  }
}
```

Human mode (default) prints `Error: <message>` to stderr; the exit code
carries the class. In `--json` mode log output is suppressed to errors so
stderr carries only the envelope (`--debug` restores full logging).

## JSON success output (schema 1)

Every payload carries `"schema": 1`. Shapes per command:

| Command | Payload |
|---|---|
| `run -d` | `{"schema":1,"id","name","detached":true}` |
| `run` (attach) | container output streams raw; process exit code carries the result |
| `stop` | `{"schema":1,"container","status":"stopped"\|"not_running"}` |
| `start` | `{"schema":1,"container","status":"started"\|"already_running"}` |
| `rm` | `{"schema":1,"container","status":"removed"}` |
| `exec` | `{"schema":1,"container","exit_code"}` |
| `logs` | `{"schema":1,"container","content"}` |
| `pull` | `{"schema":1,"image","cached",…}` |
| `images` | `{"schema":1,"images":[{repository,tag,registry}…]}` |
| `ps`/`list` | JSON array of container objects |
| `doctor`/`events`/`backend info`/`gpu list`/`vm status`/`secret list`/`volume ls`/`volume inspect` | per-command objects (pre-existing) |

Lifecycle `status` strings are additive-only: `stopped`, `not_running`,
`started`, `already_running`, `removed`.

## Container exit codes vs. exo exit codes

Today a container that exits non-zero surfaces as exit 6 (`Container exited
with code N`). Propagating the *container's own* exit code through `exo run`
(à la `docker run`) is planned follow-up work — the taxonomy above describes
exo/runtime failures only.

## Conversion status

Typed at the command boundary: `run`, `stop`, `start`, `rm`, `exec`, `logs`,
`pull`, `import`, backend selection — all with `--json` success output and the
error envelope. `exo-mac` raises typed `CONTAINER_NOT_FOUND`/`SECRET_NOT_FOUND`;
`exo-vm-mac` raises typed `BACKEND_UNSUPPORTED` for resource limits, GPU, and
host bind mounts. Errors raised through `anyhow` chains keep their code
(recovered via downcast at the process boundary). Known transitional gaps:

- `exo-image` registry errors are stringly with embedded HTTP statuses;
  `pull` maps 404 → `IMAGE_NOT_FOUND`, other statuses → 6. Typed errors in
  `exo-image` are the follow-up.
- `daemon`, `vm`, `secret`, `volume`, `events` commands still exit 6 on most
  failures.
- Guest-agent errors inside `exo-vm-mac` (`GuestResponse::Error`) are stringly
  — typed errors over the guest RPC channel are the follow-up.
- `exo-wsl` internals return stringly errors.
- `run` (attach) has no JSON success payload by design: container output must
  not be corrupted. Failures still produce the envelope.
