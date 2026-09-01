# Exo — Roadmap

> Rewritten 2026-08-23 from a code-level survey (the previous roadmap dated 2026-03-09 predated the entire microVM track and no longer described reality). Historical version lives in git history.
>
> **Goal:** a stable container runtime that competes with Docker/Podman for **agent workloads**. The consumer of this CLI is an AI agent (via Orchestre), not a human — so "compatibility" means an *agent contract*, not docker flag parity.

## Strategy (decided 2026-08-23)

1. **Both backends, macOS leads.** exo-vm-mac is the wedge (Docker Desktop is weak on macOS: licensing, perf, battery; VM-level isolation beats runc for agent sandboxes). Linux rootless stays green in CI.
2. **Exo's own CLI, agent-first.** No docker CLI/socket emulation. What agents need: stable JSON schemas, typed errors, idempotent verbs, zero prompts.

## Current state (code-grounded, 2026-08-23)

### Parity matrix

| Capability | Linux | macOS native | macOS microVM | WSL |
|---|---|---|---|---|
| run / stop / rm / ps | ✅ | ✅ | ✅ | ⚠️ broken (wrong binary name) |
| exec / logs | ❌ placeholders that fake success | ✅ | ✅ | partial |
| images / pull | ✅ | ✅ | ✅ | partial |
| Volume mounts | ✅ bind+named | ✅ | ⚠️ guest-only, host bind rejected | partial |
| Port mapping | ⚠️ needs socat/ncat | ❌ host net only | ✅ TCP/UDP vsock tunnels | ✅ netsh |
| Resource limits | ✅ cgroup v2 | ❌ ignored | ❌ rejected | delegated |
| `--json` output | ⚠️ read commands only | ⚠️ | ⚠️ | ⚠️ |
| Crash reconciliation | ✅ reconciler | n/a | ✅ VM daemon | n/a |
| `ExoBackend` trait | ❌ bypassed | ✅ | ✅ | ❌ shell wrapper |

### Known defects (fix-first, before any new features)

- ~~**D1** — Linux `exec`/`logs` are stubs that print args and return `Ok`~~ **FIXED 2026-08-23**: placeholders now fail with `BACKEND_UNSUPPORTED` (exit 4); real Linux impls tracked in B3
- **D2** — WSL invokes `exo-runtime`; the binary is `openclaw-runtime` (`crates/exo-wsl/src/command.rs`)
- ~~**D3** — Every error exits 1; anyhow stringly errors, no typed codes~~ **FIXED 2026-08-23**: `ExoError` taxonomy + exit-code harness (`docs/EXIT_CODES.md`); untyped legacy errors exit 6
- ~~**D4** — `run`, `stop`, `rm`, `exec`, `logs`, `images`, `pull` have no `--json`~~ **FIXED 2026-08-23**: global `--json` flag with schema-1 payloads on all lifecycle commands + JSON error envelope on stderr
- **D5** — `path.chars().next().unwrap()` panics on empty path (exo-wsl); `.unwrap()` on mutex locks (windows_networking.rs)
- **D6** — cgroup `Drop` silently swallows cleanup errors → leaks cgroup subtrees
- **D7** — TOCTOU race in daemon auto-start (socket check → connect)
- **D8** — Linux doesn't implement `ExoBackend` → cross-backend drift

---

## Pillar 1 — Agent contract CLI

*The interface an LLM can drive without guessing. Each item is testable.*

- [x] **A1. Typed errors.** `thiserror` error taxonomy crate-wide (`ImageNotFound`, `DaemonUnreachable`, `ContainerRunning`, `InvalidName`, `BackendUnsupported`, …). Kill stringly `anyhow!` at command boundaries. **DONE 2026-08-31** — 2026-08-23: `ExoError` + `exit_code_for` harness in `exo-runtime/src/error.rs`; core lifecycle commands converted. 2026-08-31 remainders: `exo-image` raises typed `IMAGE_NOT_FOUND`/`REGISTRY_AUTH`/`REGISTRY_UNAVAILABLE` from HTTP statuses + transport errors; `validate_reference` fails malformed refs fast with `INVALID_INPUT`; `daemon`/`vm`/`secret`/`volume`/`events` commands typed (incl. transitional `map_daemon_error` string mapper); new variants `RegistryAuth`/`RegistryUnavailable`/`ContainerExited`. Remaining stringly: backend internals (`exo-mac`/`exo-vm-mac` guest RPC, `exo-wsl`) behind documented sniff-mappers until protocols carry typed codes.
- [x] **A2. Exit-code taxonomy.** Documented, stable: 0 ok, 2 not-found, 3 conflict/state, 4 backend-unavailable, 5 invalid-input, 6 internal. Mapped from A1. **DONE 2026-08-31** — 2026-08-23: contract in `docs/EXIT_CODES.md`; `main` returns typed `ExitCode` (never 1). 2026-08-31: container exit-code propagation landed — attach-mode `run`/`exec`/`start --attach` exit with the workload's own code (clamped 1..=255), envelope `CONTAINER_EXITED` disambiguates it from exo failures; verified live on the microVM (`sh -c "exit 42"` → exit 42).
- [x] **A3. `--json` everywhere.** Every command accepts `--json`; errors emit `{"error": {"code": "...", "message": "...", "retryable": bool}}` on stdout with the A2 exit code. **DONE 2026-08-23** (deviation: envelope on **stderr**, not stdout — stdout stays pure data): global `--json` flag (per-command dupes removed), schema-1 success payloads on `run -d`/`stop`/`start`/`rm`/`exec`/`logs`/`pull`/`images`, `envelope_for` in the main harness, log noise suppressed in json mode. Shapes documented in `docs/EXIT_CODES.md`.
- [x] **A4. Schema versioning.** Every JSON payload carries `"schema": 1`. Additive-only changes within a version. **DONE 2026-08-23** — enforced by the shared `print_json` helper, which inserts the field so call sites can't forget.
- [x] **A5. Idempotent verbs.** `stop`/`rm` on absent or stopped containers succeed (or exit 2 with a typed code — pick one, document it, test it). Agents retry; retries must be safe. **DONE 2026-08-26** — semantics: desired-state success (`stop` on stopped → Ok `not_running`, `start` on running → Ok `already_running`), existence always validated (absent → 2). Documented matrix in `docs/EXIT_CODES.md`; vm-mac guest errors mapped to typed codes via transitional `map_guest_error` (+ host-side idempotent-start intercept); CLI contract tests in `crates/exo/tests/cli_contract.rs` (8 tests, platform-portable).
- [x] **A6. No fake success.** Placeholders (D1) either work or exit 4 with `BACKEND_UNSUPPORTED`. Never `Ok` on a no-op. **DONE 2026-08-23** — fixed with D1 in the A3/A4 chunk (Linux exec/logs placeholders now exit 4).
- [x] **A7. Generated agent docs.** Command/flag/JSON-schema reference generated from clap definitions, checked by CI so docs can't drift. **DONE 2026-09-01** — `exo agent-docs` (hidden meta command) renders the full reference from the live clap tree via introspection (`crates/exo/src/agent_docs.rs`); committed as `docs/AGENT_CLI.md`. Drift check = `agent_docs_match_committed_reference` in cli_contract.rs (fails when a command/flag changes without regenerating). JSON payload shapes live in the generator's `JSON_SHAPES` table as their single source of truth.

## Pillar 2 — Stability engineering

- [ ] **S1. Executable conformance suite.** Port `test.sh` categories (smoke/isolation/features/edge/integration) to Rust integration tests driving the real binary; assert on JSON output + exit codes, not stdout prose.
- [ ] **S2. CI matrix on every commit.** macOS (vm-mac), Linux rootless, WSL2. Today only WSL2 has CI — and its backend is broken (D2).
- [ ] **S3. Crash-resilience tests.** Kill VM/daemon mid-run → restart → state reconciles (Linux reconciler exists; vm-mac needs equivalent guest-state recovery). Stale containers cleaned up.
- [ ] **S4. Nightly soak.** Hundreds of run/stop cycles; watch fd counts, cgroup leaks (D6), memory.
- [ ] **S5. Defect sweep.** D2, D5, D6, D7 fixed with regression tests.
- [ ] **S6. Linux implements `ExoBackend`** (D8) — one trait, four backends, parity enforced by the conformance suite running against each.

## Pillar 3 — Backend completion (macOS leads)

- [ ] **B1. vm-mac resource limits.** Map `ContainerConfig.resources` to guest cgroups (currently rejected outright).
- [ ] **B2. vm-mac host bind mounts.** Streamed/virtio-fs host paths into guest (currently rejected; Orchestre workspaces need this).
- [ ] **B3. Linux `exec`/`logs` for real** (unblocks D1).
- [ ] **B4. Linux port mapping without socat/ncat** — built-in forwarder or documented hard dependency with `doctor` check.
- [ ] **B5. Default seccomp profile on Linux** (audit of current filter vs docker's default).
- [ ] **B6. Repair or formally drop WSL** (D2) — a broken backend shipped is worse than none.
- [ ] **B7. Keep the parity matrix above generated from conformance results**, not hand-maintained.

## Non-goals (for now)

- Docker socket/API emulation, Compose compatibility
- Swarm/k8s orchestration features
- docker CLI flag-parity aliases

## Gates

- **G5 (agent-usable):** A1–A7 done — **MET 2026-09-01.** Orchestre can drive exo with typed-error handling, no stderr parsing, and a generated CLI reference that can't drift from the binary.
- **G6 (trustworthy):** S1–S3 green in CI on macOS + Linux.
- **G7 (feature-complete core):** B1–B3 done; parity matrix has no ❌ in the Linux/vm-mac columns for the core lifecycle.

*Next review: after G6.*
