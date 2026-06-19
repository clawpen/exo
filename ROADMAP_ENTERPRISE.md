# Exo Enterprise Roadmap

> From **agent runtime** → **Docker-replacement product**.
> Branch: `exo-enterprise`. Created 2026-06-19.

## Thesis

Exo already has the hard part: a working single-host runtime (namespaces, cgroups
v2, seccomp, capability drop, rootless, overlayfs, OCI pull, GPU, lifecycle daemon
at 1000+ container scale, WSL2). What's missing to be a *product* is **distribution,
build, local networking, and proof** — plus polish.

**Two things we are NOT building** (they already exist in `F:\Software\Claw Pen`'s
orchestrator and are the control-plane's job, not the runtime's):

| Owned by Claw Pen orchestrator | File |
| --- | --- |
| Cross-host overlay mesh (Tailscale/WireGuard/ZeroTier) | `orchestrator/src/network.rs` |
| Secret management / rotation | `orchestrator/src/secret_manager.rs` |
| Volume attachment & orchestration | `orchestrator/src/volume_attachment.rs` |
| Service discovery / service registry | `orchestrator/src/api.rs` |
| Multi-agent scaling, workflows, teams, snapshots | `orchestrator/src/{executor,teams,snapshots,workflow}.rs` |

Exo provides the *primitives* these consume (the mount, the local veth, the env
injection point); it must not reimplement the orchestration on top of them.

---

## Headline: Space savings & efficiency vs Docker

The differentiator we lead with. Measured by `scripts/bench-vs-docker.sh` →
`results/bench-*.json`. Four cost-mapped axes:

1. **Disk footprint** — content-addressed layer store with cross-image dedup.
2. **Cold spawn** — daemonless/light-daemon path vs dockerd round-trip.
3. **Idle control-plane RSS** — exo daemon vs `dockerd` baseline.
4. **Density** — concurrent containers per GB RAM.

### E0 — Prove it (do this first; it's the sales story)
- [x] `scripts/bench-vs-docker.sh` harness (disk / spawn / idle RSS / summary JSON)
- [ ] Density sub-benchmark: ramp N containers, record RSS/container & failure point
- [ ] CI job publishing `results/bench-*.json` as artifacts per commit (regression gate)
- [ ] `docs/benchmarks.md` with reproducible methodology + a results table
- [ ] One-shot reproducer container so external users can verify our claims

Where the wins come from (validate each with the harness, don't assume):
- Content-addressed dedup across shared base layers (E1)
- No HTTP/dockerd per-op overhead; agent stdio bus instead of REST
- Rootless + minimal daemon → low idle RSS
- Smaller agent-tuned base images

---

## E1 — Image distribution & storage *(biggest functional gap)*
OCI-compatible at the boundary so we inherit the whole ecosystem.
- [x] OCI registry pull + Docker Hub auth + multi-arch (done)
- [x] **Content-addressed layer store** — extract each layer once into `layers/<digest>/`,
      image→layers index for refcounting, hardlink-composed per-image rootfs
      (shared inodes = real dedup, works rootless without overlay). `crates/exo-image/src/cas.rs`
- [x] `exo system df` — physical vs logical bytes + dedup % (drives the benchmark)
- [x] `exo system prune` — GC layers no image references
- [x] `exo push` to any OCI registry — pull,push-scoped token, HEAD-skip
      existing blobs, monolithic blob upload, manifest PUT by tag
- [x] `exo rmi` — unregister image + auto-prune its now-orphaned layers
- [x] `exo image inspect` — per-layer size, refcount, shared-vs-exclusive disk
- [ ] Optional: zstd layer compression + lazy/stargz pull (further space + cold-start win)
- [ ] Follow-up: switch rootfs composition to overlay lowerdirs where the kernel
      allows it (zero-copy vs hardlink), keeping hardlink as the rootless fallback

## E2 — Build  [IN PROGRESS]
- [x] **Agent manifest** (`exo.toml`) parser + validation — declares base image,
      build steps, tools, resource budget, default-deny egress policy
      (`crates/exo-image/src/manifest.rs`, 4 tests; `examples/exo.toml`)
- [x] `exo build [-f exo.toml]` — resolves/validates manifest, pulls base, prints plan
- [x] **Layer-commit primitive** — tar a dir into a content-addressed CAS layer
      (`LayerStore::commit_layer`, idempotent, 1 test)
- [x] Execute **COPY** steps → commit a layer → register + compose the built image
      (built images dedup their base layer automatically; verified end-to-end)
- [x] Generate OCI config (ENV/CMD/workdir + uncompressed diff_ids) + manifest so
      built images are `push`able (`crates/exo-image/src/oci_build.rs`, 1 test;
      verified end-to-end: built image yields a valid OCI manifest + config)
- [ ] Execute RUN steps via the runtime's container-exec path *(needs live machine)*
- [x] Dockerfile subset (FROM/RUN/COPY/ADD/ENV/CMD/WORKDIR, line continuations,
      exec+shell forms) as alternate input — `exo build -f Dockerfile -t name`
      (3 tests; verified end-to-end; inherits default-deny egress)
- [x] `.exoignore` — gitignore-lite matcher excludes node_modules/.git/secrets
      from COPY (size + secret-leak hygiene). `crates/exo-image/src/ignore.rs`,
      3 tests; verified end-to-end
- [ ] Build cache keyed on step inputs

## E3 — Local networking (single-host primitives only)
> Mesh/discovery stays in Claw Pen. Exo provides the local plumbing it can't.
- [ ] Bridge (`exo0`) + veth pair + IP allocation per container
- [ ] `-p host:container` publish via nftables
- [ ] Container-local DNS + `/etc/hosts`
- [ ] `network mode` already in CLI — wire up `bridge|host|none` backends

## E4 — Runtime completeness
- [ ] Full writable overlay via fuse-overlayfs (finish the rootless gap from old roadmap)
- [ ] `exo stats` — live cgroup metrics (CPU/mem/io)
- [ ] Healthcheck primitive (`--health-cmd`) + status surfaced in `list`
- [ ] `--restart` policies (no/on-failure/always) in the daemon reconciler
- [ ] `exo cp`, `exo inspect`, `exo events` (events partially present)

## E5 — Programmability & trust (enterprise table stakes)
- [ ] Stable, versioned daemon API + SDK (formalize the existing Unix socket protocol)
- [ ] Image signing/verification (cosign/sigstore) + SBOM emission
- [ ] Vulnerability scan hook on pull/build
- [ ] Prometheus `/metrics` endpoint on the daemon
- [ ] Structured audit log + log rotation
- [ ] Single-binary installer + package repos; Docker→Exo migration guide

## E6 — Compose-lite (optional, only if Claw Pen doesn't cover it)
- [ ] `exo-compose.yml` for multi-container *local* dev stacks (agent + vector DB + tools)
- [ ] Defer real orchestration to Claw Pen; this is a dev-loop convenience only

## E7 — Production hardening pass *(gate before any GA/enterprise claim)*
A dedicated security/robustness sweep, not folded into feature work. Done when an
external pentest and a fuzzing run come back clean and the checklist below is met.

### Supply-chain & content integrity
- [x] Layer blob digest verification before extraction into the CAS (`cas.rs`)
- [x] Tar extraction confined to target dir (no path-traversal escape via `unpack_in`)
- [x] Reject symlink-escape at compose time (parent-traverses-symlink guard +
      higher-layer dir overrides a lower-layer symlink). `cas.rs`, unix test
- [x] Per-layer uncompressed size cap (decompression-bomb guard, `EXO_MAX_LAYER_BYTES`)
- [x] Manifest layer-count + total-size caps on pull (`EXO_MAX_LAYERS`,
      `EXO_MAX_IMAGE_BYTES`) — OOM/DoS guard. `integrity.rs`
- [x] Verify config digest + manifest↔config (layer/diff_id count) on pull.
      `integrity.rs`, 3 tests; real pulls still pass
- [ ] Enforce image signature verification (cosign) as a gate, not just emit (ties to E5)

### Concurrency & state integrity
- [x] File-locked, crash-safe index mutations (concurrent `pull`/`rmi`/`prune`
      no longer lose updates; stale-lock steal after 30s). `cas.rs`, concurrency test
- [x] `exo system check [--repair]` — detect dangling image→layer refs + orphan
      layers, repair by unregistering dangling images and pruning. `cas.rs`, 1 test
- [ ] Fsync + atomic-rename audit across all on-disk writes
- [ ] Quotas: max store size, per-image disk cap, eviction policy

### Runtime isolation hardening
- [ ] Seccomp/AppArmor/SELinux profile review + default-deny baseline
- [ ] Capability-set audit (drop-all default, opt-in adds only)
- [ ] User-namespace / uid-gid mapping review for the rootless path
- [ ] Resource-exhaustion limits enforced (pids, memory, fds) even under daemon scale
- [ ] No-new-privileges, read-only rootfs option, masked /proc paths

### Process & operational security
- [ ] Daemon socket authn/authz + permission hardening (no world-writable socket)
- [ ] Secret-handling review (no secrets in logs, env dumps, or event log)
- [x] Dependency audit (`cargo audit` + `cargo deny`) wired into CI as a gate
      (`.github/workflows/ci.yml` security job, `deny.toml`)
- [ ] Clean up legacy warnings, then flip CI fmt/clippy from informational to `-D warnings`
- [ ] Fuzz layer extraction, manifest parsing, and reference parsing (cargo-fuzz)
- [x] Threat model doc + documented trust boundaries (`docs/threat-model.md`,
      "Saboteur" adversary; maps each attack class to its control)
- [ ] Run `/security-review` on the branch and resolve findings before tagging

---

## Sequencing

1. **E0** — benchmark harness + docs (proof first, it guides everything)
2. **E1** — CAS dedup + push (unlocks the disk-savings claim *and* distribution)
3. **E3 / E4** — local networking + runtime completeness (feature parity)
4. **E2** — build + agent manifest (differentiation)
5. **E5** — trust/observability (enterprise readiness)
6. **E6** — compose-lite only if a gap remains
7. **E7** — production hardening pass; **gates GA** (don't claim "production" until clean)

## Principles
- OCI-compatible at every boundary; original engineering only on agent-native parts.
- Never duplicate Claw Pen's control plane — provide primitives, not orchestration.
- Every efficiency claim must be reproducible via `scripts/bench-vs-docker.sh`.
- Keep rootless default; maintain CLI backward compatibility; test Linux + WSL2.
