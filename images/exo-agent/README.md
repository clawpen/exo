# Exo Agent Container Images

## Variants

| Image | Size | Features |
|-------|------|----------|
| `exo-agent:latest` | ~25MB | Full agent with SQLite memory + tools |
| `exo-agent:slim` | ~8MB | LLM client only, no persistence |
| `openclaw-agent:latest` | ~500MB | Full OpenClaw instance |

## Build

```bash
# Standard (recommended)
./images/exo-agent/build.sh

# Ultra-slim
./images/exo-agent/build.sh slim

# Both
./images/exo-agent/build.sh all
```

## Run

```bash
# Interactive chat
podman run -it --rm \
    -e ZAI_API_KEY=xxx \
    exo-agent:latest

# With workspace mount
podman run -it --rm \
    -e ZAI_API_KEY=xxx \
    -v ./workspace:/workspace \
    exo-agent:latest

# Custom config
podman run -it --rm \
    -e ZAI_API_KEY=xxx \
    -v ./my-config.toml:/app/config.toml \
    exo-agent:latest
```

## Configuration

The config file is TOML (not JSON):

```toml
[agent]
name = "my-agent"

[llm]
provider = "zai"
model = "glm-4.7-flash"

[prompt]
system = "You are helpful."
```

## File Structure

```
images/exo-agent/
├── Containerfile        # Standard build (~25MB)
├── Containerfile.slim   # Minimal build (~8MB)
├── agent.toml          # Default config
├── build.sh            # Build script
└── README.md           # This file
```

## Comparison with OpenClaw

| Metric | Exo Agent | OpenClaw |
|--------|-----------|----------|
| Image size | 25MB | 500MB |
| Startup | <1s | 3-5s |
| Memory | 64MB | 512MB |
| Tools | 5 basic | 50+ |
| Channels | stdio | 10+ platforms |
| Plugins | No | Yes |

Use **exo-agent** for fast, lightweight agents.
Use **OpenClaw** for full-featured agents with integrations.
