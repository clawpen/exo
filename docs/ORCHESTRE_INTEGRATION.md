# Orchestre Integration Contract

This document describes the stable CLI/JSON contract Orchestre should use to
start, monitor, and audit Exo multi-agent runs.

The integration point is intentionally lightweight: Orchestre launches
`exoclaw orchestrate-run`, passes a single JSON input file, receives a single
JSON output object, and reads durable run state from disk.

## Goals

- Keep Exo's daemon/runtime path light and fast.
- Let Orchestre supply a prime directive and success criteria without linking
  to Exo internals.
- Let agents run as built-ins, arbitrary commands, or `exo run` workloads.
- Persist enough state for inspection, auditing, and later resume work.
- Preserve a simple agent protocol: prompt JSON on stdin, report JSON on stdout.

## CLI entrypoint

```bash
exoclaw orchestrate-run \
  --json-input /path/to/input.json \
  --state-dir /path/to/orchestrations \
  --json
```

Arguments:

- `--json-input PATH`: reads the stable request JSON described below. If this is
  omitted, the positional directive and CLI flags are converted into the same
  input shape internally.
- `--state-dir DIR`: stores run state under this directory. If omitted, Exo uses
  `EXO_ORCHESTRATION_DIR`, then `EXO_STATE_DIR/orchestrations`, then
  `~/.local/share/exo/orchestrations`, then a temp fallback.
- `--run-id ID`: optional CLI-side run id when not using `--json-input`.
- `--json`: prints the final `OrchestrateRunOutput` JSON object to stdout.

## Request JSON: `OrchestrateRunInput`

Minimum request:

```json
{
  "objective": "Ship lightweight Exo agent workflow",
  "success_criteria": [
    "planner completed",
    "builder completed",
    "verifier completed"
  ],
  "constraints": ["keep daemon light"],
  "executor": { "type": "builtin" },
  "run_id": "orch-demo-001",
  "max_rounds": 24
}
```

Fields:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `objective` | string | yes | Prime directive given to all initial agents. |
| `success_criteria` | string[] | no | All criteria must be covered by succeeded report summaries. Defaults to `[]`. |
| `constraints` | string[] | no | Constraints injected into agent prompts. Defaults to `[]`. |
| `executor` | object | no | Executor config. Defaults to `{ "type": "builtin" }`. |
| `run_id` | string/null | no | Durable run id. If omitted, Exo generates `orch-<uuid>`. |
| `max_rounds` | number | no | Coordinator prompt budget before blocking. Defaults to `24`. |

Success matching is deliberately simple for now: each success criterion is split
into alphanumeric words, words of length `<= 3` are ignored, and every remaining
word must appear in the combined summaries of succeeded reports.

## Executor configs

### Built-in executor

Use for wiring tests and no-LLM smoke tests.

```json
{ "type": "builtin" }
```

The built-in executor returns succeeded reports with summaries like
`planner completed`, `builder completed`, and `verifier completed`.

### Command executor

Use when Orchestre wants to run a local worker command directly.

```json
{
  "type": "command",
  "command": "python3 /path/to/agent_worker.py"
}
```

The command is executed as `sh -c <command>` for each agent task. It receives an
`AgentPrompt` JSON object on stdin and should print an `AgentReport` JSON object
on stdout. The runner reads the last stdout line that starts with `{` and parses
it as the report.

### Exo executor

Use when agents should be spawned through Exo.

```json
{
  "type": "exo",
  "exo_bin": "/absolute/path/to/exo",
  "backend": "native",
  "image": "host",
  "agent_command": "cat",
  "volumes": ["/host/workspace:/workspace"],
  "secrets": ["OPENAI_API_KEY"],
  "sandbox": "off"
}
```

Fields:

| Field | Default | Notes |
| --- | --- | --- |
| `exo_bin` | `exo` | Exo CLI binary path. |
| `backend` | `native` | Backend passed to `exo run --backend`. |
| `image` | `host` | Image/name passed to `exo run`. |
| `agent_command` | `cat` | Shell command run inside the Exo workload as `sh -c <agent_command>`. |
| `volumes` | `[]` | Repeated `SRC:DEST` mount specs passed as `-v`. |
| `secrets` | `[]` | Secret names passed as `--secret`. |
| `sandbox` | `null` | Optional sandbox mode, e.g. `off`, `auto`, or `required`. |

The Exo executor injects these environment variables into each workload:

- `EXO_TASK_ID`
- `EXO_AGENT_ID`

## Agent process protocol

### Prompt on stdin: `AgentPrompt`

Each command/Exo agent receives one JSON object on stdin:

```json
{
  "task_id": "task-1",
  "agent_id": "planner",
  "prompt": "Prime directive:\n..."
}
```

Fields:

| Field | Type | Notes |
| --- | --- | --- |
| `task_id` | string | Coordinator task id. |
| `agent_id` | string | Role id such as `planner`, `builder`, or `verifier`. |
| `prompt` | string | Full prompt including objective, role, criteria, constraints, and context. |

### Report on stdout: `AgentReport`

Agents should print a report JSON object to stdout. Extra logs are allowed as
long as the final JSON report is on a line beginning with `{`.

```json
{
  "task_id": "task-1",
  "status": "succeeded",
  "summary": "planner completed the implementation plan",
  "artifacts": ["docs/plan.md"],
  "followups": ["implementation should add mailbox event log"]
}
```

Fields:

| Field | Type | Notes |
| --- | --- | --- |
| `task_id` | string | May be empty; the runner overwrites it with the assigned task id. |
| `status` | enum | `pending`, `running`, `succeeded`, `failed`, or `blocked`. Agents normally return `succeeded`, `failed`, or `blocked`. |
| `summary` | string | Human-readable result. Used for success-criteria matching. |
| `artifacts` | string[] | Paths, URLs, or labels for produced artifacts. Defaults to `[]`. |
| `followups` | string[] | Follow-up prompts. The coordinator assigns each to the best matching agent. Defaults to `[]`. |

If no valid report JSON is found, the command executor treats exit code `0` as a
succeeded report with summary `<agent_id> completed (exit 0)` and a non-zero exit
as a failed report.

## Response JSON: `OrchestrateRunOutput`

With `--json`, stdout is a single JSON object:

```json
{
  "run_id": "orch-demo-001",
  "outcome": {
    "status": "succeeded",
    "rounds": 3,
    "message": "All success criteria are covered by agent reports."
  },
  "state_path": "/path/to/orchestrations/orch-demo-001/state.json",
  "events_path": "/path/to/orchestrations/orch-demo-001/events.jsonl",
  "mailbox_path": "/path/to/orchestrations/orch-demo-001/mailbox.jsonl",
  "state": {
    "directive": { "objective": "...", "success_criteria": [], "constraints": [], "max_rounds": 24 },
    "agents": [],
    "tasks": [],
    "reports": [],
    "round": 3,
    "status": "succeeded"
  }
}
```

Outcome fields:

| Field | Type | Notes |
| --- | --- | --- |
| `status` | enum | `running`, `succeeded`, `blocked`, or `failed`. |
| `rounds` | number | Number of agent prompts executed. |
| `message` | string | Terminal reason or success summary. |

The embedded `state` is the same object persisted in `state.json` inside a
`RunRecord` wrapper.

## Sleep/resume model

The mailbox/event log is what lets agents sleep and come back to their work.
An agent or Orchestre should record a `checkpoint` or `sleep` event before it
stops doing work. When it wakes up, it reads `state.json` plus mailbox events
after the last sequence number it processed and rebuilds its local context.

Current durable pieces:

- `state.json`: latest coordinator state and final/in-progress outcome.
- `input.json`: resolved original request/executor config used as resume default.
- `mailbox.jsonl`: ordered inter-agent/coordinator events with sequence numbers.
- `mailbox.seq`: last reserved mailbox sequence number.
- `.append.lock`: short-lived append lock used to serialize lifecycle/mailbox writes.
- `events.jsonl`: coarse lifecycle audit events such as `started`/`finished`.
- `artifacts/`: files produced by agents or copied in by Orchestre.

This is not full automatic resume yet; it is the durable substrate for resume.
`exoclaw orchestrate-resume` now handles the mechanical part: it reloads
`state.json`, re-queues pending/interrupted/failed tasks, appends `run_resumed`
to `mailbox.jsonl`, and continues the run. Orchestre should own the policy part:
when to wake a run, which executor to use, whether to override prompts, and how
to chain goals.

## Persistent run-state layout

For run id `orch-demo-001` and state dir `/path/to/orchestrations`:

```text
/path/to/orchestrations/
└── orch-demo-001/
    ├── state.json
    ├── input.json
    ├── events.jsonl
    ├── mailbox.jsonl
    ├── mailbox.seq
    └── artifacts/
```

### `state.json`

`state.json` contains a `RunRecord`:

```json
{
  "run_id": "orch-demo-001",
  "state": {
    "directive": {
      "objective": "Ship lightweight Exo agent workflow",
      "success_criteria": ["planner completed"],
      "constraints": ["keep daemon light"],
      "max_rounds": 24
    },
    "agents": [],
    "tasks": [],
    "reports": [],
    "round": 3,
    "status": "succeeded"
  },
  "outcome": {
    "status": "succeeded",
    "rounds": 3,
    "message": "All success criteria are covered by agent reports."
  }
}
```

The file is written once at run start with `outcome: null`, after each accepted
agent report with the latest in-progress state, and again at run completion with
the final state and outcome.

### `input.json`

`input.json` contains the resolved `OrchestrateRunInput`, including the generated
`run_id` and executor config. `orchestrate-resume` uses this executor by default
unless Orchestre passes an override such as `--agent-cmd` or `--use-exo`.

### `events.jsonl`

`events.jsonl` is append-only JSON Lines for coarse run lifecycle audit events.
Current event types:

```jsonl
{"timestamp_ms":1783451898216,"run_id":"orch-demo-001","event_type":"started","message":"orchestration run started"}
{"timestamp_ms":1783451898216,"run_id":"orch-demo-001","event_type":"finished","message":"Succeeded"}
```

### `mailbox.jsonl`

`mailbox.jsonl` is the append-only inter-agent event log. Every event has a
monotonic `sequence` so an agent can persist "last seen sequence N", sleep, and
later read only events where `sequence > N`. Appends are serialized with a
short-lived lock file and use `mailbox.seq` to avoid re-scanning the full log on
every write.

Example events:

```jsonl
{"sequence":1,"timestamp_ms":1783451898216,"run_id":"orch-demo-001","event_id":"evt-...","kind":"run_started","from":"coordinator","message":"orchestration run started","payload":{"objective":"..."}}
{"sequence":2,"timestamp_ms":1783451898217,"run_id":"orch-demo-001","event_id":"evt-...","kind":"task_prompted","from":"coordinator","to":"planner","task_id":"task-1","message":"coordinator prompted planner","payload":{"prompt":"Prime directive:..."}}
{"sequence":3,"timestamp_ms":1783451898218,"run_id":"orch-demo-001","event_id":"evt-...","kind":"agent_report","from":"planner","to":"coordinator","task_id":"task-1","message":"planner reported Succeeded","payload":{"summary":"planner completed"}}
```

Built-in runner event kinds today:

| Kind | Producer | Meaning |
| --- | --- | --- |
| `run_started` | coordinator | Run state was initialized. |
| `task_prompted` | coordinator | A prompt was assigned to an agent. |
| `agent_report` | agent/runner | An agent report was accepted. |
| `handoff_requested` | agent/runner | A report requested follow-up work. |
| `run_finished` | coordinator | The runner reached succeeded/blocked/failed. |
| `run_resumed` | coordinator | A persisted run was loaded and resumed. |

External Orchestre/agent event kinds can include `message`, `checkpoint`,
`sleep`, `wake`, `artifact`, or any other stable string.

### Run inspection CLI

List persisted runs:

```bash
exoclaw orchestrate-list \
  --state-dir /path/to/orchestrations \
  --json
```

Show one run, optionally including audit and mailbox logs:

```bash
exoclaw orchestrate-status orch-demo-001 \
  --state-dir /path/to/orchestrations \
  --include-events \
  --include-mailbox \
  --json
```

### Resume CLI

Resume with the original persisted executor config:

```bash
exoclaw orchestrate-resume orch-demo-001 \
  --state-dir /path/to/orchestrations \
  --json
```

Resume with a fixed/overridden direct command executor:

```bash
exoclaw orchestrate-resume orch-demo-001 \
  --state-dir /path/to/orchestrations \
  --agent-cmd 'python3 /path/to/agent_worker.py' \
  --json
```

Resume through Exo:

```bash
exoclaw orchestrate-resume orch-demo-001 \
  --state-dir /path/to/orchestrations \
  --use-exo \
  --exo-backend native \
  --exo-image host \
  --exo-agent-cmd 'python3 /workspace/agent_worker.py' \
  --volume "$PWD:/workspace" \
  --json
```

### Event-log CLI

Append one durable event:

```bash
exoclaw event-log append \
  --run-id orch-demo-001 \
  --state-dir /path/to/orchestrations \
  --kind checkpoint \
  --from-agent planner \
  --to-agent builder \
  --task-id task-1 \
  --payload-json '{"last_file":"docs/plan.md"}' \
  "planner is sleeping after drafting the plan"
```

Read all events after a checkpoint sequence:

```bash
exoclaw event-log list \
  --run-id orch-demo-001 \
  --state-dir /path/to/orchestrations \
  --since 12 \
  --agent builder \
  --json
```

### `artifacts/`

Reserved for files produced by agents or copied by Orchestre. Today Exo creates
the directory but does not populate it automatically.

## Smoke test

```bash
tmpdir=$(mktemp -d /tmp/exo-orch.XXXXXX)
cat > "$tmpdir/input.json" <<'JSON'
{
  "objective": "Confirm stable JSON orchestration API and persistent run state for Orchestre",
  "success_criteria": [
    "planner completed",
    "builder completed",
    "verifier completed"
  ],
  "executor": { "type": "builtin" },
  "run_id": "orch-smoke-json",
  "max_rounds": 24
}
JSON

exoclaw orchestrate-run \
  --json-input "$tmpdir/input.json" \
  --state-dir "$tmpdir/state" \
  --json

cat "$tmpdir/state/orch-smoke-json/events.jsonl"
cat "$tmpdir/state/orch-smoke-json/mailbox.jsonl"

exoclaw event-log append \
  --run-id orch-smoke-json \
  --state-dir "$tmpdir/state" \
  --kind sleep \
  --from-agent planner \
  --payload-json '{"last_seen_sequence":3}' \
  "planner sleeping until more work arrives"

exoclaw event-log list \
  --run-id orch-smoke-json \
  --state-dir "$tmpdir/state" \
  --agent planner \
  --json

exoclaw orchestrate-list \
  --state-dir "$tmpdir/state" \
  --json

exoclaw orchestrate-status orch-smoke-json \
  --state-dir "$tmpdir/state" \
  --include-mailbox \
  --json

exoclaw orchestrate-resume orch-smoke-json \
  --state-dir "$tmpdir/state" \
  --json
```

Expected outcome: `outcome.status` is `succeeded`, and `state.json`,
`input.json`, `events.jsonl`, `mailbox.jsonl`, and `mailbox.seq` exist under the
run directory.

## Current limitations / next integration work

- The coordinator runs tasks sequentially today; concurrent agents are a future
  extension.
- Resume-after-failure is not implemented yet, but the state layout is ready for
  it.
- `orchestrate-resume` is mechanical resume, not goal policy. Orchestre should
  decide when/why to resume and whether to override the executor.
- Real LLM `exo-agent` workers still need to be wired behind the Exo executor.
