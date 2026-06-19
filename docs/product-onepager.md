# Exo + Maestro — one-pager

> **Run AI agents like containers. Control them like a swarm.**

## The problem

AI agents aren't microservices. They spawn sub-agents, call tools, consume tokens,
and multiply — swarms, hives, things that grow like a culture and get **out of
control**. General-purpose runtimes (Docker, containerd) were built for stateless
services, not for autonomous processes you need to *contain, watch, and stop*.

## The product

Two layers, one clean seam (a stable API — never a shared codebase):

| | What it is | Job |
| --- | --- | --- |
| **Exo** | Agent-first container **runtime** | The shell that contains each agent: isolation, images, GPU, fast spawn — rootless by default |
| **Maestro** | The **control plane / UI** | Your window into the runtime: conduct, observe, and kill the swarm at scale |

Exo provides the primitives; Maestro does the orchestration. Either can be renamed
or shipped independently because they only meet at the API.

## Why it beats Docker for agents

- **Containment, not just isolation** — per-tool sandboxing and egress policy for
  processes that actively try to spread.
- **Space efficiency** — content-addressed layer store: every layer is extracted
  **once** and shared across images (hardlinked, works rootless). `exo system df`
  reports the dedup savings; `exo image inspect` shows shared-vs-exclusive disk.
- **Fast, dense spawn** — light daemon, agent stdio bus instead of HTTP round-trips;
  built for 1000+ concurrent agents per host.
- **A kill switch** — when a swarm goes wrong, scuttle it on command ("sabord").
- **OCI-compatible at the edges** — `exo pull`/`push` speak any OCI registry, so you
  inherit the whole ecosystem instead of reinventing it.

## Proof, not claims

Every efficiency number is reproducible: `scripts/bench-vs-docker.sh` measures disk
footprint, cold spawn, idle control-plane memory, and density against Docker on the
same host, and emits machine-readable results for CI regression tracking.

## Status

Runtime is real today: isolation (namespaces/cgroups/seccomp/caps), OCI pull **and
push**, content-addressed dedup store, GPU, lifecycle daemon at scale, Windows/WSL2.
See `ROADMAP_ENTERPRISE.md` for what's next (local networking, build + agent
manifest, signing/observability).

---
*Exo: from outside, the protective shell that makes agents possible. Maestro: the
hand that conducts them.*
