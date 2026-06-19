# Images & build

How Exo pulls, builds, stores, and ships agent images. Every example below is
from a real run; numbers will vary with the images you use.

## Pull (with automatic dedup)

```bash
exo pull python:3.12-slim
exo pull python:slim          # shares its base layer with the above
```

Exo stores each layer **once**, content-addressed. Images that share a base layer
share it on disk — no duplicate copies like a naive per-image rootfs.

## See the space savings

```bash
$ exo system df
Images:                5
Unique layers:         10
Physical (on disk):    245.7 MiB
Logical (no dedup):    324.4 MiB
Reclaimed by dedup:    78.7 MiB (24% saved)
```

`physical` is what's actually on disk; `logical` is what it would cost if every
image kept its own copy of every layer. The gap is the dedup win.

## Inspect one image

```bash
$ exo image inspect python:slim
Image:   docker.io/library/python:slim
Total:   117.5 MiB
Exclusive: 38.8 MiB (rest shared with other images)

LAYER                        SIZE  SHARED BY
sha256:72c03230f136      78.7 MiB  2 images
sha256:da106fed7af0       3.6 MiB  exclusive
...
```

`Exclusive` is the disk you'd actually reclaim by removing this image; shared
layers stay until nothing references them.

## Build an agent image

Two inputs, one build path. Both produce a normal OCI image whose base layer is
automatically deduped against everything else in the store.

### Agent manifest (`exo.toml`) — recommended

The agent-native alternative to a Dockerfile: it declares not just the filesystem
but the agent's tools, resource budget, and a **default-deny egress** policy.

```toml
[agent]
name = "researcher"
from = "python:3.12-slim"

[build]
workdir = "/app"
copy = [["./src", "/app"]]
cmd  = ["python", "main.py"]

[tools]
allow = ["bash", "web"]

[egress]
allow = ["api.anthropic.com"]   # nothing else is reachable
```

```bash
exo build                 # uses ./exo.toml
exo build -f path/exo.toml
```

### Dockerfile

Existing Dockerfiles work too — and inherit Exo's default-deny egress.

```bash
exo build -f Dockerfile -t my-agent
```

Supported instructions: `FROM`, `RUN`*, `COPY`/`ADD`, `ENV`, `CMD`, `WORKDIR`
(line continuations and both exec/shell forms). *RUN execution is in progress.

### .exoignore

Keep junk and secrets out of the image (smaller layers, no leaked secrets):

```
node_modules
.git
*.log
secrets.env
```

## Ship it

```bash
exo push ghcr.io/me/researcher:latest
```

Exo speaks the OCI distribution API, so it pushes to any registry (Docker Hub,
GHCR, ECR, …). Blobs the registry already has are skipped.

## Maintenance

```bash
exo rmi python:slim       # refcount-aware: only this image's exclusive layers are reclaimed
exo system prune          # drop layers no image references
exo system check          # scan for dangling refs / orphan layers
exo system check --repair # ...and fix them
```

## Integrity & limits (defaults; override via env)

Exo verifies and bounds untrusted image content on pull/extract — see
[threat-model.md](threat-model.md) for the full picture.

| Concern | Default | Override |
| --- | --- | --- |
| Per-layer uncompressed size | 10 GiB | `EXO_MAX_LAYER_BYTES` |
| Max layers per manifest | 1024 | `EXO_MAX_LAYERS` |
| Max total image size | 50 GiB | `EXO_MAX_IMAGE_BYTES` |

Layer digests are verified before extraction, config↔manifest consistency is
checked on pull, and layer extraction is confined against path-traversal and
symlink-escape.
