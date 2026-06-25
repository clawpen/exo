# Exo Enterprise Roadmap

> From **agent runtime** → **Docker-replacement product**.
> Branch: `exo-enterprise`. Created 2026-06-19.
> Last audited: 2026-06-24. `[~]` = partially implemented; see inline notes.

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
2. **Cold spawn** — lightweight-daemon path vs dockerd round-trip.
3. **Idle control-plane RSS** — exo daemon vs `dockerd` baseline.
4. **Density** — concurrent containers per GB RAM.

### E0 — Prove it (do this first; it's the sales story)
- [x] `scripts/bench-vs-docker.sh` harness (disk / spawn / idle RSS / summary JSON)
- [x] Density sub-benchmark: ramp N containers, record RSS/container & failure point
- [x] CI job publishing `results/bench-*.json` as artifacts per commit (regression gate)
- [x] `docs/benchmarks.md` with reproducible methodology + a results table
- [x] One-shot reproducer container so external users can verify our claims

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
- [~] Optional: zstd layer compression + lazy/stargz pull (further space + cold-start win) *(zstd decompression on pull implemented; lazy/stargz pull deferred — requires FUSE daemon + HTTP Range client)*
- [x] Follow-up: switch rootfs composition to overlay lowerdirs where the kernel
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
- [ ] Execute RUN steps via the runtime's container-exec path *(parsed but skipped; needs live machine)*
- [x] Dockerfile subset (FROM/RUN/COPY/ADD/ENV/CMD/WORKDIR, line continuations,
      exec+shell forms) as alternate input — `exo build -f Dockerfile -t name`
      (3 tests; verified end-to-end; inherits default-deny egress)
- [x] `.exoignore` — gitignore-lite matcher excludes node_modules/.git/secrets
      from COPY (size + secret-leak hygiene). `crates/exo-image/src/ignore.rs`,
      3 tests; verified end-to-end
- [ ] Build cache keyed on step inputs

## E3 — Local networking (single-host primitives only)
> Mesh/discovery stays in Claw Pen. Exo provides the local plumbing it can't.
- [x] Bridge (`exo0`) + veth pair + IP allocation per container (`crates/exo-runtime/src/network.rs`; `ip` command based, JSON lease file in state dir)
- [x] `-p host:container` publish via nftables (per-container `ip exo_<id>` table with DNAT/masquerade; `nft` command based)
- [x] Container-local DNS + `/etc/hosts` (`/etc/resolv.conf` and `/etc/hosts` injected into rootfs for `bridge`/`none` modes; inter-container entries from IPAM leases)
- [x] `network mode` already in CLI — wire up `bridge|host|none` backends (`NetworkMode` enum, netns isolation dispatch, parent-side bridge/veth setup)

## E4 — Runtime completeness
- [x] Full writable overlay via fuse-overlayfs *(kernel overlay attempted first, fuse-overlayfs fallback via `std::process::Command`; read-only fallback uses first lowerdir instead of returning `Err`)*
- [x] `exo stats` — live cgroup metrics + CLI (`crates/exo/src/commands/stats.rs`; daemon request/response)
- [x] Healthcheck primitive (`--health-cmd`, `--health-interval`, `--health-timeout`, `--health-retries`, `--health-start-period`) + status surfaced in `exo list` and events
- [x] `--restart` policies (`no`/`on-failure`/`always`/`on-daemon-restart`) in CLI + daemon reconciler with exponential backoff
- [x] `exo cp`, `exo inspect`, `exo events` *(events + `image inspect` already existed; container `inspect` and `cp` added)*

## E5 — Programmability & trust (enterprise table stakes)
- [x] Stable, versioned daemon API + SDK (formalize the existing Unix socket protocol; `API_VERSION = "1"`, `DaemonRequestEnvelope`/`DaemonResponseEnvelope` with version validation; clients in `run` and `stats` updated)
- [x] Image signing/verification (cosign/sigstore) + SBOM emission (`crates/exo-runtime/src/sbom.rs` and `sign.rs`; `syft`/`trivy` SBOM generation, `cosign verify` before pull, `cosign sign` after push; `--verify`, `--cosign-key`, `--sbom`, `--sign` flags; `EXO_VERIFY`/`EXO_SBOM`/`EXO_SIGN` env vars)
- [x] Vulnerability scan hook on pull/build (`crates/exo-runtime/src/scan.rs`; grype/trivy shell-out on composed rootfs; `--skip-scan` flag; `EXO_SKIP_SCAN`/`EXO_SCAN_FATAL` env vars; JSON reports saved to `<image-root>/scans/`)
- [x] Prometheus `/metrics` endpoint on the daemon (`crates/exo/src/metrics.rs`; Prometheus text format on `127.0.0.1:9090`, configurable via `EXO_METRICS_ADDR`, sampled every `EXO_METRICS_INTERVAL_MS`)
- [x] Structured audit log + log rotation *(SQLite ring-buffer in `events.rs`; trimmed rows are archived to timestamped JSONL files in `events-archive/`, capped at `MAX_ARCHIVE_FILES`; added `EventLog::export` and `exo events --export PATH`)*
- [x] Single-binary installer + package repos; Docker→Exo migration guide (`scripts/install.sh`, `scripts/build-release.sh`, `packaging/homebrew/exo.rb`, `packaging/scoop/exo.json`; `docs/DOCKER_TO_EXO_MIGRATION.md`; install section in `README.md`)

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
- [~] Fsync + atomic-rename audit across all on-disk writes *(atomic temp-file + rename for index/layers; no systematic fsync audit)*
- [x] Store-size quota (`EXO_MAX_STORE_BYTES`, default 100 GiB) — extract bails
      with guidance rather than auto-evicting mid-pull. `cas.rs`, 1 test
- [ ] LRU/last-used eviction policy (auto-reclaim instead of hard fail)

### Runtime isolation hardening
- [~] Seccomp/AppArmor/SELinux profile review + default-deny baseline *(seccomp whitelist/blacklist exists; no AppArmor/SELinux profiles)*
- [~] Capability-set audit (drop-all default, opt-in adds only) *(capability enum/drop functions exist; no formal audit doc)*
- [~] User-namespace / uid-gid mapping review for the rootless path *(UidMap/GidMap + setup code exist; no documented review)*
- [~] Resource-exhaustion limits enforced (pids, memory, fds) even under daemon scale *(cgroup v2 memory/pids/cpu; no fd/rlimit enforcement)*
- [~] No-new-privileges, read-only rootfs option, masked /proc paths *(no_new_privs + readonly_rootfs implemented; masked_paths declared in AgentProfile but not enforced in rootfs.rs)*

### Process & operational security
- [ ] Daemon socket authn/authz + permission hardening (no world-writable socket) *(socket currently chmod'd to 0o777)*
- [x] Secret-handling review of exo-image: no secrets logged anywhere; added
      redacting Debug impls for RegistryAuth/DockerConfigAuth so future `{:?}`
      logs can't leak credentials. `registry.rs`, 1 test
- [~] Extend secret-leak review to daemon/runtime (events, env dumps) *(image crate done; daemon/runtime not systematically reviewed)*
- [x] Dependency audit (`cargo audit` + `cargo deny`) wired into CI as a gate
      (`.github/workflows/ci.yml` security job, `deny.toml`)
- [~] Clean up legacy warnings, then flip CI fmt/clippy from informational to `-D warnings` *(`-D warnings` only for `exo-image`; workspace fmt/clippy still `continue-on-error: true`)*
- [x] Deterministic robustness tests: adversarial input to reference / exo.toml /
      Dockerfile / .exoignore parsers — assert no panic (`reference.rs`, `manifest.rs`,
      `ignore.rs`)
- [ ] Continuous fuzzing harness (cargo-fuzz) over the same entry points
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
