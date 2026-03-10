# OpenClaw Agent Image Builder
# Creates a minimal OCI image for running OpenClaw agents

## Quick Start

```bash
# Build with Docker
./build.sh

# Import into exo (once supported)
exo import openclaw-agent-latest.tar
```

## Manual Build

```bash
# Build the image
docker build -t openclaw-agent:latest -f Containerfile .

# Export for exo
docker save openclaw-agent:latest | gzip > openclaw-agent-latest.tar.gz
```

## Image Contents

- Node.js 22
- OpenClaw CLI
- Agent workspace structure
- Default skills and tools

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| OPENCLAW_WORKSPACE | /agent | Workspace directory |
| PORT | 18789 | Gateway port |
| NODE_ENV | production | Node environment |

## Ports

- **18789**: OpenClaw gateway API

## Volumes

- `/agent/memory`: Agent memory storage
- `/agent/.openclaw`: OpenClaw config
