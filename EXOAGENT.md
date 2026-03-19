# ExoAgent Branch

This branch extends Exo with a full agent gateway system, providing an **OpenClaw-compatible** runtime with containerized skill execution.

## What's New

### exo-gateway (crate)
WebSocket-based agent gateway with:

- **Protocol v1.0.0** - JSON message protocol for agent communication
- **Session Management** - Track agent connections with activity timeouts
- **Skill Registry** - Container and WASM skill support with YAML manifests
- **Cron Scheduler** - Time-based job execution with tokio-cron-scheduler
- **REST API** - HTTP endpoints for external integration
- **WebSocket /ws** - Real-time bidirectional agent communication

### exo-agent (binary)
CLI tool for running the gateway:

```bash
# Start gateway
exo-agent gateway --bind 0.0.0.0:8080 --skills-dir ./skills

# List skills
exo-agent list-skills

# Create skill template
exo-agent new-skill my-skill --output ./skills

# Invoke tool (WIP)
exo-agent invoke my-skill my-tool --args '{"key": "value"}'
```

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                      exo-agent                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │   Gateway    │  │   Skills     │  │   Cron       │       │
│  │   Server     │  │   Registry   │  │   Jobs       │       │
│  └──────────────┘  └──────────────┘  └──────────────┘       │
└──────────────────────────────────────────────────────────────┘
                              │
┌──────────────────────────────────────────────────────────────┐
│                    exo-gateway                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │   WebSocket  │  │   Session    │  │   Tool       │       │
│  │   Handler    │  │   Manager    │  │   Bus        │       │
│  └──────────────┘  └──────────────┘  └──────────────┘       │
└──────────────────────────────────────────────────────────────┘
                              │
┌──────────────────────────────────────────────────────────────┐
│                    exo-runtime                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │   Container  │  │   GPU        │  │   Namespace  │       │
│  │   Runtime    │  │   Passthrough│  │   Isolation  │       │
│  └──────────────┘  └──────────────┘  └──────────────┘       │
└──────────────────────────────────────────────────────────────┘
```

## Protocol Example

```json
// Agent → Gateway
{
  "type": "tool_request",
  "request_id": "uuid",
  "skill": "bash",
  "tool": "exec",
  "args": {"command": "ls -la"},
  "timeout_ms": 30000
}

// Gateway → Agent
{
  "type": "tool_response",
  "request_id": "uuid",
  "result": {
    "type": "success",
    "output": {"stdout": "...", "stderr": ""},
    "execution_time_ms": 150
  }
}
```

## Skill Manifest (skill.yaml)

```yaml
name: web-search
version: "1.0.0"
description: "Search the web using DuckDuckGo"

runtime:
  type: container
  image: exo-skill-websearch:latest
  resources:
    memory: "512M"
    cpu: 0.5
    gpu: false

tools:
  - name: search
    description: "Search the web"
    parameters:
      type: object
      properties:
        query:
          type: string
      required: ["query"]
    timeout_ms: 30000
```

## Status

| Component | Status | Notes |
|-----------|--------|-------|
| Gateway Server | ✅ | WebSocket + REST API working |
| Session Management | ✅ | Activity tracking, cleanup |
| Skill Registry | ✅ | YAML manifest loading |
| Cron Scheduler | ✅ | Time-based job execution |
| Protocol | ✅ | v1.0.0 message types |
| CLI Binary | ✅ | exo-agent commands |
| **Tool Execution** | 🔧 | Needs exo-runtime integration |
| **GPU Passthrough** | 🔧 | Needs testing with LLM containers |
| **WASM Skills** | 📋 | Not yet implemented |

## Next Steps

1. **Integrate exo-runtime** for actual container execution
2. **Add shell/REPL mode** for interactive development
3. **Implement LLM provider** for local model management
4. **Build skill marketplace** (discover, install skills)
5. **Add authentication** (API keys, JWT)

## Running

```bash
# Terminal 1: Start gateway
cd /root/exo
cargo run -p exo-agent -- gateway --bind 127.0.0.1:8080

# Terminal 2: Connect via wscat
npx wscat -c ws://127.0.0.1:8080/ws
> {"type": "hello", "version": "1.0.0", "agent_id": "test", "capabilities": []}
```

## Comparison with OpenClaw

| Feature | OpenClaw | ExoAgent |
|---------|----------|----------|
| Language | TypeScript/Node | Rust |
| Sandboxing | Process-based | Container + namespaces |
| GPU Support | External | Native passthrough |
| Skills | Functions | Containers/WASM |
| Protocol | Proprietary | Open (documented) |
| Self-hosted | Yes | Yes (binary) |
