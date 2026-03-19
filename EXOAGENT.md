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
| **Tool Execution** | ✅ | Via docker/podman (exo-runtime integration pending) |
| **Shell/REPL** | ✅ | Interactive shell with command history |
| **Local LLM** | ✅ | Ollama integration with chat, embeddings, model pull |
| **GPU Passthrough** | 🔧 | Available for LLM containers |
| **WASM Skills** | 📋 | Not yet implemented |

## Next Steps

1. ~~Integrate exo-runtime for actual container execution~~ (docker/podman bridge working)
2. ~~Add shell/REPL mode~~ (✅ Complete)
3. **Implement LLM provider** for local model management with GPU passthrough
4. **Build skill marketplace** (discover, install skills)
5. **Add authentication** (API keys, JWT)

## LLM Integration (Local Models)

Exo Agent includes built-in support for local LLM inference via Ollama:

### Quick Start

```bash
# 1. Start gateway with LLM skill
cargo run -p exo-agent -- gateway \
  --bind 127.0.0.1:8080 \
  --skills-dir ./skills

# 2. Pull a small model (in another terminal)
docker run --rm -p 11434:11434 ollama/ollama

# Or use the shell to pull via the gateway
exo-agent shell
curl http://localhost:11434/api/pull -d '{"name":"qwen2.5:0.5b"}'
```

### LLM Tools

| Tool | Description |
|------|-------------|
| `llm.chat` | Generate chat completions |
| `llm.list_models` | List available local models |
| `llm.pull` | Download a model from registry |
| `llm.embeddings` | Generate text embeddings |

### Example: Chat Completion

```bash
exo-agent shell
exo> /call llm chat '{"model":"qwen2.5:0.5b","messages":[{"role":"user","content":"Hello!"}]}'
```

Or via WebSocket:
```json
{
  "type": "tool_request",
  "request_id": "1",
  "skill": "llm",
  "tool": "chat",
  "args": {
    "model": "qwen2.5:0.5b",
    "messages": [
      {"role": "user", "content": "What is Rust?"}
    ],
    "temperature": 0.7
  }
}
```

### GPU Passthrough

Enable GPU for LLM inference in `skills/llm/skill.yaml`:

```yaml
runtime:
  type: container
  image: ollama/ollama:latest
  resources:
    memory: "8G"
    cpu: 4.0
    gpu: true  # ← Enable GPU
```

The container will be started with `--gpus all` flag.

### Architecture

```
┌─────────────┐    HTTP API    ┌─────────────────┐
│   Agent     │ ─────────────→ │  Ollama Server  │
│  (shell)    │                │   (container)   │
└─────────────┘                └─────────────────┘
                                      │
                                      ↓
                              ┌───────────────┐
                              │  Local Model  │
                              │  (GPU/CPU)    │
                              └───────────────┘
```

The LLM skill uses a direct HTTP bridge to communicate with Ollama, bypassing stdin/stdout for better streaming support.

## Interactive Shell/REPL

Exo Agent includes a full interactive shell for testing and development:

```bash
# Connect to a running gateway
exo-agent shell --url ws://127.0.0.1:8080/ws

# Or with a custom agent ID
exo-agent shell --agent-id my-test-agent
```

### Shell Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/ping` | Send ping to gateway |
| `/skills` | Request skills list |
| `/tools` | Request tools list |
| `/call <skill> <tool> [args]` | Call a tool |
| `/exit` | Exit the shell |

### Example Session

```
╔═══════════════════════════════════════╗
║      Exo Agent Interactive Shell      ║
╚═══════════════════════════════════════╝

Connecting to ws://127.0.0.1:8080/ws...
Connected!

Type /help for available commands

exo> /help
Available Commands:
  /help, /h - Show this help
  /skills, /s - List available skills
  /tools, /t - List available tools
  /call, /c <skill> <tool> [args] - Call a tool
  /ping - Send ping to server
  /exit, /quit, /q - Exit the shell

exo> /ping
►─ Ping sent
◄─ Pong!

exo> /call time now
►─ Calling time:now...
[1a2b3c4d] ✓ Success
{
  "timestamp": 1700000000,
  "iso": "2026-03-20T05:30:00Z",
  "local": "2026-03-20 13:30:00 CST"
}

exo> /exit
Disconnecting...
Goodbye!
```

### Raw JSON Messages

You can also send raw JSON messages for debugging:

```
exo> {"type": "tool_request", "request_id": "1", "skill": "time", "tool": "now", "args": {}}
```

## Running

```bash
# Terminal 1: Start gateway with skills
cd /root/exo
cargo run -p exo-agent -- gateway \
  --bind 127.0.0.1:8080 \
  --skills-dir ./skills

# Terminal 2: Connect via wscat
npx wscat -c ws://127.0.0.1:8080/ws
> {"type": "hello", "version": "1.0.0", "agent_id": "test", "capabilities": []}

# Terminal 2: Call a tool (using builtin skill)
> {"type": "tool_request", "request_id": "1", "skill": "time", "tool": "now", "args": {}}
< {"type": "tool_response", "request_id": "1", "result": {...}}
```

## Testing Tool Execution

### Builtin Skills (no container needed)
The `time` skill is a builtin that works without docker:

```bash
# Start gateway
cargo run -p exo-agent -- gateway --bind 127.0.0.1:8080

# In another terminal, use wscat or similar:
echo '{"type":"hello","version":"1.0.0","agent_id":"test","capabilities":[]}' | websocat ws://127.0.0.1:8080/ws
echo '{"type":"tool_request","request_id":"1","skill":"time","tool":"now","args":{}}' | websocat ws://127.0.0.1:8080/ws
```

### Container Skills (requires docker/podman)
Container runtime temporarily uses docker/podman while exo-runtime integration is pending:

```yaml
# skills/bash/skill.yaml
name: bash
version: "1.0.0"
description: "Execute bash commands"

runtime:
  type: container
  image: alpine:latest
  resources:
    memory: "128M"
    cpu: 0.1
    gpu: false

tools:
  - name: exec
    description: "Execute a bash command"
    parameters:
      type: object
      properties:
        command:
          type: string
      required: ["command"]
    timeout_ms: 30000
```

Container execution features:
- Memory limits enforced
- CPU limits enforced  
- Network disabled by default (`--network none`)
- GPU passthrough available (`gpu: true`)
- Args passed via stdin as JSON

## Comparison with OpenClaw

| Feature | OpenClaw | ExoAgent |
|---------|----------|----------|
| Language | TypeScript/Node | Rust |
| Sandboxing | Process-based | Container + namespaces |
| GPU Support | External | Native passthrough |
| Skills | Functions | Containers/WASM |
| Protocol | Proprietary | Open (documented) |
| Self-hosted | Yes | Yes (binary) |
