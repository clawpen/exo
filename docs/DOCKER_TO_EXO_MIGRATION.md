:
# Docker to Exo Migration Guide

Exo is a container runtime built specifically for AI agents. This guide helps you migrate local development and deployment workflows from Docker to Exo.

## What Exo is (and isn't)

Exo provides Docker-compatible single-host primitives:

- `exo run`, `exo stop`, `exo rm`, `exo ps`, `exo logs`
- `exo build -f Dockerfile`, `exo build -f exo.toml`
- `exo pull`, `exo push`, `exo image inspect`
- `exo exec`, `exo cp`, `exo inspect`
- Networking: `--network bridge|host|none`, `-p host:container`
- Resource limits: `--memory`, `--cpu`, cgroup v2

Exo does **not** replace Docker Swarm, Kubernetes, or cross-host orchestration. The Claw Pen control plane covers multi-agent mesh, secrets, and volumes for production deployments.

## Command mapping

| Docker | Exo | Notes |
|--------|-----|-------|
| `docker run` | `exo run` | `--name`, `--rm`, `-d`, `-e`, `-v`, `-p`, `--network`, `--memory`, `--cpu` supported. |
| `docker ps` | `exo list` / `exo ps` | `--all`, `--json` flags available. |
| `docker stop` | `exo stop` | `--time`, `--force` supported. |
| `docker rm` | `exo remove` / `exo rm` | `--force` supported. |
| `docker logs` | `exo logs` | `--follow`, `--tail`, `--timestamps`. |
| `docker exec` | `exo exec` | `--interactive`, `--tty`, `--user`. |
| `docker cp` | `exo cp` | `container:path` syntax on either side. |
| `docker inspect` | `exo inspect` | Container metadata, live stats, mounts. |
| `docker build` | `exo build` | `-f`, `-t`; supports Dockerfiles and `exo.toml`. |
| `docker pull` | `exo pull` | `--verify`, `--cosign-key`, `--sbom`, `--skip-scan`. |
| `docker push` | `exo push` | `--sign`, `--cosign-key`. |
| `docker images` | `exo images` | Deduplication-aware disk usage. |
| `docker rmi` | `exo rmi` | Refcount-aware layer pruning. |
| `docker stats` | `exo stats` | Live cgroup metrics. |
| `docker events` | `exo events` | SQLite-backed audit log with JSONL export. |

## Key differences

### Daemon

Docker runs a heavyweight daemon (`dockerd`) that owns all state. Exo uses a **lightweight daemon** for lifecycle fan-out, reconciliation, and event persistence. Many commands (`inspect`, `cp`, `stats`, `events`) read on-disk state directly and do not require the daemon.

Start the daemon for detached runs:

```bash
exo daemon
```

### Networking

Exo provides single-host networking only:

- `bridge` (default): `exo0` bridge + veth + IPAM.
- `host`: share the host network namespace.
- `none`: isolated loopback only.

Cross-host service mesh and discovery belong in Claw Pen.

### Images

Exo uses the same OCI image format and registries as Docker, so existing images push and pull without conversion. The local store is content-addressed, so shared layers are stored once across images.

### Security

- Rootless by default via user namespaces.
- Seccomp and capability dropping built in.
- Optional image signing with `cosign` on push/pull.
- Optional SBOM generation and vulnerability scanning with `grype`/`trivy`.

## Migration checklist

1. **Install Exo** (see `README.md` or `scripts/install.sh`).
2. **Pull base images**: `exo pull python:3.12`
3. **Convert `docker run` invocations** using the command mapping above.
4. **Convert `docker build -t my-app .`** to `exo build -f Dockerfile -t my-app .`.
5. **Replace compose stacks**: Exo does not have `docker-compose`. For local multi-container stacks, use the Claw Pen orchestrator or run containers individually with `exo run`.
6. **Update CI/CD**: Replace `docker` commands with `exo` commands. Use `exo push --sign` for signed artifacts.
7. **Monitor**: `exo stats`, `exo events`, and the Prometheus `/metrics` endpoint on the daemon.

## Common issues

- **WSL2 on Windows**: Exo runs the runtime inside WSL2. Install a WSL2 distro and run `exo` from Windows; the `exo.cmd` wrapper handles the bridge.
- **User namespaces**: If kernel user namespaces are disabled, rootless mode may fall back to a less isolated path. Check `/proc/sys/kernel/unprivileged_userns_clone` on Debian/Ubuntu.
- **Capabilities**: Exo drops more capabilities than Docker by default. Use `--privileged` only when necessary.

## Getting help

- `exo --help`
- `exo <command> --help`
- See `docs/` for the threat model, benchmarks, and Windows setup.
