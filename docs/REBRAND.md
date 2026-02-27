# Rebrand: Containment → Exo

## Overview

Renaming "Containment" to "Exo" - the agent container runtime from Claw Pen.

**Why "Exo"?**
- Short, memorable, modern
- "Exoskeleton" shortened - protective outer layer
- Perfect fit with OpenClaw's crustacean theme
- Easy to type: `exo run` instead of `containment run`

## What Changes

| Before | After |
|---|---|
| `containment` (project) | `exo` |
| `containment` (CLI) | `exo` |
| `containment-runtime` (crate) | `exo-runtime` |
| `containment-image` (crate) | `exo-image` |
| `containment-wsl` (crate) | `exo-wsl` |
| `containment-gpu` (crate) | `exo-gpu` |
| `openclaw_container` | `openclaw_exo` |

## File Changes

### 1. Rename Crates

```bash
# Workspace
mv crates/containment-runtime crates/exo-runtime
mv crates/containment-image crates/exo-image
mv crates/containment-wsl crates/exo-wsl
mv crates/containment-gpu crates/exo-gpu

# CLI
mv crates/cli crates/exo-cli
```

### 2. Update Cargo.toml

```toml
[workspace]
members = [
    "exo-runtime",
    "exo-image",
    "exo-wsl",
    "exo-gpu",
    "exo-cli",
]
```

### 3. Update Package Names

```rust
// containment-runtime → exo-runtime
[package]
name = "exo-runtime"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "exo"
path = "src/main.rs"
```

### 4. Update CLI Command

```bash
# Before
containment run --image python:3.12 --tool bash

# After
exo run --image python:3.12 --tool bash
```

## Repository Changes

Consider whether to:
1. **Rename repo** on GitHub/gitea to `exo`
2. **Keep repo name** but rename the product
3. **Create new org** like `clawpen` with `exo` as the repo

## Documentation Updates

- README.md
--AGENT_MANIFESTO.md → EXO_MANIFESTO.md
- All code comments mentioning "containment"
- LICENSE if it mentions the old name

## External References

- Update any published articles/docs
- Update social media bios
- Update README badges

## Migration Guide for Users

```bash
# Old command
containment run --image ubuntu:latest

# New command
exo run --image ubuntu:latest

# Or if both coexist temporarily
exoshell run --image ubuntu:latest
```

## Brand Assets (TODO)

- Logo for Exo
- Terminal color scheme
- Website/docs styling

---

*Exo: The agent container runtime from Claw Pen*
