# macOS Viability Tracker

This file turns the macOS viability roadmap into concrete acceptance gates.

## Gate 1: Native Agent Mode

Status: **implemented / needs hardening**

- [x] Native host-process backend.
- [x] Env clearing by default.
- [x] Per-container `HOME` and `TMPDIR`.
- [x] Best-effort macOS sandbox profile.
- [x] Strict sandbox mode: `--sandbox required`.
- [x] Local secret store: `exo secret set/list/remove`.
- [x] Explicit secret injection: `exo run --secret NAME`.
- [x] Named volume store: `exo volume create/list/inspect/remove`.
- [x] Native named volumes can be used as Exo-managed host directories and
      resolved as `--workdir` mount targets.
- [x] Host readiness diagnostics: `exo doctor`.
- [x] GPU detection and Metal/MPS environment hints.
- [x] Backend capability reporting: `exo backend info`.
- [ ] Additional integration tests on an unrestricted macOS host where
      `sandbox-exec` succeeds.
- [ ] Signing/notarization and Homebrew packaging.

Acceptance command set:

```bash
exo doctor
exo secret set OPENAI_API_KEY --value "$OPENAI_API_KEY"
exo run --backend native --sandbox auto --secret OPENAI_API_KEY host -- env
exo run --backend native --sandbox required host -- true
exo run --backend native --gpu host -- env
```

## Gate 2: Backend Abstraction

Status: **started**

- [x] `exo_runtime::ExoBackend` trait exists.
- [x] Native macOS backend implements `ExoBackend`.
- [x] macOS Linux microVM backend facade implements `ExoBackend`.
- [x] Backend capabilities model exists.
- [ ] Linux runtime moved behind `ExoBackend`.
- [ ] Windows WSL backend moved behind `ExoBackend`.
- [ ] CLI commands dispatch through a backend selector instead of per-command
      `cfg` blocks.

Acceptance command set:

```bash
exo backend info --json
exo run --backend native host -- true
```

## Gate 3: Exo-managed macOS Linux microVM MVP

Status: **scaffolded / not production-ready**

- [x] `exo-vm-mac` crate exists.
- [x] Minimal guest-agent crate exists.
- [x] `exo vm init/start/status/stop/reset` commands exist.
- [x] Host/guest RPC schema includes run/list/start/stop/remove/logs/exec
      container operations.
- [x] Host/guest run spec includes mount specs and network/port intent.
- [x] `exo run --backend linux ...` routes through the macOS Linux backend
      facade and reports the persistent-bridge readiness gate.
- [x] Hidden `exo vm serve` daemon owns the live VM handle in a persistent
      process.
- [x] `exo vm start` launches the VM daemon by default; `--foreground` keeps the
      old attached behavior.
- [x] `exo vm status` and `exo vm stop` speak to the VM daemon socket.
- [x] macOS Linux backend sends guest RPC requests through the VM daemon socket.
- [x] Guest agent handles synchronous `RunContainer` by executing a command in
      the guest and returning stdout/stderr/exit code (pre-OCI, not isolated).
- [x] Guest runtime module persists container records/logs under guest state.
- [x] Guest runtime supports synchronous run, detached spawn records, list,
      stop, remove, logs, and exec at the command-runner level.
- [x] Guest runtime supports named volume directories.
- [x] Guest runtime computes bind-mount plans and applies Linux bind mounts
      when running against an existing rootfs directory.
- [x] Guest runtime records network/port intent in container summaries.
- [x] Guest runtime imports image rootfs from `.tar`/`.tar.gz` archives.
- [x] Guest runtime computes overlay layout (lower/upper/work/merged) and
      mounts overlayfs on Linux for image-backed runs.
- [x] `ImportImage` guest RPC + `exo vm import-image` command.
- [x] `exo vm install-guest-agent PATH` installs a built guest agent binary for
      embedding into the initrd with `exo vm init --force`.
- [ ] Host-to-guest RPC is reliable on real macOS Virtualization.framework.
- [ ] VM lifecycle is covered by host integration tests.
- [x] Guest agent can execute a command in the guest through the container RPC
      path.
- [ ] Guest agent can execute an Exo runtime command with OCI image
      pull/extract/rootfs/isolation.
- [ ] Host-to-guest image transfer/sharing (so `import-image` has a guest-side
      archive without manual copying).
- [ ] VM image artifacts have a build/update/distribution story.

Acceptance command set:

```bash
exo vm init
exo vm start
exo vm status
exo vm stop
```

## Gate 4: Linux Containers on macOS

Status: **not implemented**

- [ ] `exo run --backend linux alpine echo hello`.
- [ ] OCI image pull/extract inside guest, or host pull + guest transfer.
- [ ] Overlayfs-backed rootfs inside guest.
- [ ] Namespace/cgroup/seccomp isolation inside guest.
- [ ] Logs and exit codes streamed back to host.
- [ ] `list`, `stop`, `remove`, `exec`, `logs` bridged to guest.

Acceptance command set:

```bash
exo run --backend linux alpine echo hello
exo run --backend linux python:3.12 python -c 'print("hi")'
exo list --all
exo logs <container>
exo remove <container>
```

## Gate 5: Volumes and Networking

Status: **not implemented**

- [ ] Explicit bind mounts from macOS host to guest container.
- [ ] Readonly/read-write mount modes.
- [x] Named volume CLI/store on the host.
- [ ] TCP port forwarding.
- [ ] Forwarding cleanup on stop/remove.
- [ ] Metadata display for published ports.

Acceptance command set:

```bash
exo run --backend linux -v "$PWD:/app" -w /app python:3.12 python app.py
exo run --backend linux -p 8080:8000 python:3.12 python -m http.server 8000
```

## Gate 6: Distribution

Status: **not implemented**

- [ ] Release profile build.
- [ ] Code signing.
- [ ] Notarization.
- [ ] Homebrew tap.
- [ ] `exo doctor` included in install docs.
- [ ] CI job on macOS for native mode.
- [ ] Optional CI/manual job on macOS Virtualization.framework hardware for VM
      mode.

## Gate 7: Agent Orchestration

Status: **implemented / needs Orchestre hardening**

- [x] Lightweight orchestration state machine (`PrimeDirective`,
      `AgentTask`, `AgentReport`, `Orchestrator`).
- [x] Default planner/builder/verifier roles.
- [x] Runner abstraction (`AgentExecutor`) that drives prompts to reports.
- [x] Built-in executor for wiring and no-LLM runs.
- [x] Command executor that sends prompt JSON on stdin and reads report JSON on
      stdout.
- [x] Exo-backed executor that spawns agents through `exo run`, passing task
      and agent IDs through explicit env vars.
- [x] `exoclaw orchestrate` prints initial prompts.
- [x] `exoclaw orchestrate-run` drives a full local runner loop.
- [x] `exoclaw orchestrate-run --use-exo` can spawn agent commands through
      native `exo run`.
- [x] Stable `orchestrate-run --json-input ... --json` contract documented
      for Orchestre.
- [x] Run inspection commands (`exoclaw orchestrate-list`,
      `exoclaw orchestrate-status`).
- [x] Persist orchestration state to disk (`state.json`, `events.jsonl`, `artifacts/`).
- [x] `exo-agent run-once` speaks AgentPrompt -> AgentReport and can run via
      command or Exo executors.
- [x] `exo-agent run-once` extracts fenced/multiline AgentReport JSON from
      live model output.
- [x] Live-provider validation for real `exo-agent` LLM workers as Exo processes
      using Kimi for Coding.
- [ ] Harden success detection beyond naive keyword coverage.
- [x] Agent-to-agent durable mailbox/event log (`mailbox.jsonl`, `exoclaw event-log`).
- [x] Locked mailbox appends with persistent sequence counter (`mailbox.seq`).
- [x] Mechanical resume after failure/restart (`exoclaw orchestrate-resume`).
- [ ] Orchestre-owned goal-level resume policy.

Acceptance command set:

```bash
exoclaw orchestrate "Ship lightweight Exo agent workflow"
exoclaw orchestrate-run "Ship lightweight Exo agent workflow" \
  -s "planner complete" \
  -s "builder complete" \
  -s "verifier complete"
exoclaw orchestrate-run "Ship lightweight Exo agent workflow" \
  --use-exo \
  --exo-backend native \
  --exo-image host \
  --exo-agent-cmd 'printf "{\"task_id\":\"\",\"status\":\"succeeded\",\"summary\":\"%s completed\",\"artifacts\":[],\"followups\":[]}\n" "$EXO_AGENT_ID"' \
  -s "planner completed" \
  -s "builder completed" \
  -s "verifier completed"
tmpdir=$(mktemp -d /tmp/exo-orch.XXXXXX)
cat > "$tmpdir/input.json" <<'JSON'
{
  "objective": "Confirm stable JSON orchestration API and persistent run state for Orchestre",
  "success_criteria": ["planner completed", "builder completed", "verifier completed"],
  "executor": { "type": "builtin" },
  "run_id": "orch-smoke-json",
  "max_rounds": 24
}
JSON
exoclaw orchestrate-run --json-input "$tmpdir/input.json" --state-dir "$tmpdir/state" --json
test -f "$tmpdir/state/orch-smoke-json/state.json"
test -f "$tmpdir/state/orch-smoke-json/input.json"
test -f "$tmpdir/state/orch-smoke-json/events.jsonl"
test -f "$tmpdir/state/orch-smoke-json/mailbox.jsonl"
test -f "$tmpdir/state/orch-smoke-json/mailbox.seq"
exoclaw event-log append --run-id orch-smoke-json --state-dir "$tmpdir/state" \
  --kind sleep --from-agent planner --payload-json '{"last_seen_sequence":3}' \
  "planner sleeping until more work arrives"
exoclaw event-log list --run-id orch-smoke-json --state-dir "$tmpdir/state" \
  --agent planner --json
exoclaw orchestrate-list --state-dir "$tmpdir/state" --json
exoclaw orchestrate-status orch-smoke-json --state-dir "$tmpdir/state" \
  --include-mailbox --json
exoclaw orchestrate-resume orch-smoke-json --state-dir "$tmpdir/state" --json
cat > "$tmpdir/input-agent-command.json" <<JSON
{
  "objective": "Confirm exo-agent run-once command worker",
  "success_criteria": ["planner completed", "builder completed", "verifier completed"],
  "executor": { "type": "command", "command": "$(pwd)/target/debug/exo-agent run-once --mock" },
  "run_id": "orch-agent-command-smoke",
  "max_rounds": 24
}
JSON
exoclaw orchestrate-run --json-input "$tmpdir/input-agent-command.json" \
  --state-dir "$tmpdir/agent-state" --json
cat > "$tmpdir/input-agent-exo.json" <<JSON
{
  "objective": "Confirm exo-agent run-once Exo worker",
  "success_criteria": ["planner completed", "builder completed", "verifier completed"],
  "executor": {
    "type": "exo",
    "exo_bin": "$(pwd)/target/debug/exo",
    "backend": "native",
    "image": "host",
    "agent_command": "$(pwd)/target/debug/exo-agent run-once --mock",
    "sandbox": "off"
  },
  "run_id": "orch-agent-exo-smoke",
  "max_rounds": 24
}
JSON
exoclaw orchestrate-run --json-input "$tmpdir/input-agent-exo.json" \
  --state-dir "$tmpdir/exo-agent-state" --json
```
