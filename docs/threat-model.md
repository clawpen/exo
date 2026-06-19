# Exo threat model

> Working assumption: **every agent could be a Saboteur.** Exo is the containment
> shell; this document names what a malicious agent (or a malicious image it pulls)
> tries to do, and which control stops it.

This is a living document tied to the **E7 hardening pass** in `ROADMAP_ENTERPRISE.md`.
A control marked ✅ is implemented and tested; ⏳ is planned.

## The Saboteur (adversary)

A rogue or compromised agent — or a poisoned image it runs — that wants to:
- **Spread** beyond its shell (escape the rootfs, reach the host or other agents).
- **Exfiltrate** data over the network.
- **Exhaust** host resources (disk, memory, pids) to deny service.
- **Tamper** with what other agents pull or run (supply-chain poisoning).

We assume the agent's *own* process is untrusted from the start. The trust boundary
is the container shell: the host, the daemon, the image store, and other agents are
inside the trust boundary; the agent's runtime and any pulled image content are not.

## Trust boundaries

```
        UNTRUSTED                         |        TRUSTED
  agent process / pulled image content    |  host · exo daemon · image store · other agents
----------------------------------------- + -----------------------------------------------
  tar layers, manifests, configs   ──────▶|  exo-image: verify before use
  syscalls, network egress         ──────▶|  exo-runtime: namespaces/seccomp/caps/egress
  filesystem writes                ──────▶|  rootfs confinement + compose guards
```

## Attack classes → controls

### 1. Supply-chain poisoning (tampered image content)
| Attack | Control | Status |
| --- | --- | --- |
| Layer blob swapped for malicious content | sha256 digest verified before extraction (`cas.rs::verify_blob_digest`) | ✅ |
| Config swapped / mismatched to manifest | `verify_config_consistency` (digest + layer↔diff_id count) on pull (`integrity.rs`) | ✅ |
| Unsigned/untrusted publisher | cosign signature verification as a pull gate | ⏳ |
| Vulnerable dependency in Exo itself | `cargo audit` + `cargo deny` CI gate (`deny.toml`) | ✅ |

### 2. Container/rootfs escape (spread to host)
| Attack | Control | Status |
| --- | --- | --- |
| Tar path traversal (`../../etc`) | extraction confined via `unpack_in` (`cas.rs`) | ✅ |
| Symlink in a lower layer redirects a higher layer's write onto the host | compose refuses entries whose parent traverses a symlink; higher dir overrides lower symlink (`cas.rs::parent_has_symlink`) | ✅ |
| Namespace/cap escape at runtime | user/pid/net/mount namespaces, capability drop, seccomp (`exo-runtime`) | ✅ (runtime) |
| Writable-overlay escape (rootless) | fuse-overlayfs path | ⏳ |

### 3. Resource exhaustion (denial of service)
| Attack | Control | Status |
| --- | --- | --- |
| Decompression bomb (tiny gzip → TBs) | per-layer uncompressed cap (`EXO_MAX_LAYER_BYTES`, `cas.rs`) | ✅ |
| Manifest with millions of layers / huge total | `ManifestLimits` count + size caps on pull (`EXO_MAX_LAYERS`, `EXO_MAX_IMAGE_BYTES`) | ✅ |
| Memory/CPU/pid exhaustion at runtime | cgroup v2 limits (`exo-runtime`) | ✅ (runtime) |
| Unbounded store growth | store-size quota + eviction | ⏳ |

### 4. Data exfiltration (spread out)
| Attack | Control | Status |
| --- | --- | --- |
| Agent phones home to an arbitrary host | default-deny egress; only manifest `[egress].allow` hosts reachable | ✅ (policy in manifest; enforcement ⏳ at runtime) |
| Secret baked into a pushable image | `.exoignore` excludes secrets from the build context (`ignore.rs`) | ✅ |
| Secret leaked via logs/events | secret-handling review (no secrets in logs/env dumps) | ⏳ |

### 5. State corruption (poison the store for others)
| Attack | Control | Status |
| --- | --- | --- |
| Concurrent ops lose/clobber index updates | file-locked, crash-safe index mutations (`cas.rs::with_lock`) | ✅ |
| Crashed process wedges the store | 30s stale-lock steal | ✅ |
| Store drifts into an inconsistent state | `exo system check [--repair]` detects dangling refs + orphan layers | ✅ |
| Torn writes on crash | atomic temp-file + rename for index/blobs; fsync audit | ⏳ |

## Residual risk / explicitly out of scope

- **A trusted, signed image that is itself malicious** — signature verification proves
  provenance, not safety. Runtime isolation (class 2/3) is the backstop.
- **Side channels** (timing, cache) between co-located agents — not addressed.
- **Kernel 0-days** defeating namespaces/seccomp — defense-in-depth only.

## How to extend this doc

When adding a security control, add the row here with the attack it blocks and the
`file::function` that implements it, and link the E7 checklist item. A control without
a named attack in this table is a control without a rationale.
