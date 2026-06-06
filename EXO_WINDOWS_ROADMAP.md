# Exo Windows Support Roadmap

## Current Situation

### What We Have
- **Orchestrator**: Running on Windows, managing agents
- **Docker**: Running natively on Windows (Docker Desktop)
- **Exo**: Linux-only runtime, cloned but won't build on Windows
- **ExoClient**: Already implemented in orchestrator, calls `exo` binary via CLI

### The Problem

**Exo is Linux-only** because it uses:
- Linux namespaces (user, pid, net, mount, cgroup)
- Cgroups v2 for resource management
- Direct syscalls via libc
- Seccomp for syscall filtering
- chroot/pivot_root for filesystem isolation

**These don't exist on Windows** - there's no direct Windows equivalent.

### What We're Doing Now

```
Windows Host → Docker Desktop (Windows) → Linux Containers
                      ↑
                   Heavy, slow, overhead
```

## The Gap Analysis

### 1. **Build System Gap**
- ❌ Exo won't compile on Windows (libc, nix crate dependencies)
- ✅ WSL2 backend stub exists in `crates/exo-wsl/` but incomplete
- ❌ No Windows exo binary

### 2. **Communication Gap**
- ❌ Windows orchestrator can't spawn exo processes directly
- ❌ No IPC mechanism between Windows and WSL2 exo runtime
- ✅ WSL interop exists (`wsl.exe command`) but not integrated

### 3. **Performance Gap**
- Docker: ~2-3 second startup time per container
- Exo promise: Millisecond startup (daemonless)
- **But**: WSL2 adds overhead to every call

### 4. **Feature Gap - What Exo Has That Docker Doesn't**
- ✅ Agent-native communication (stdio + tool bus vs HTTP)
- ✅ Tool-level sandboxing (each tool gets its own security context)
- ✅ Daemonless architecture (no persistent daemon overhead)
- ✅ Designed for AI agents (not microservices)

## Recommended Architecture

### The Hybrid Approach (Realistic Path)

```
┌─────────────────────────────────────────────────────────────┐
│ Windows Host                                                │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  Claw Pen Orchestrator (Rust, Windows native)        │   │
│  │  - Manages agents                                      │   │
│  │  - Handles WebSocket/chat                             │   │
│  │  - Volume management                                  │   │
│  └──────────────────────────────────────────────────────┘   │
│                          │                                   │
│                          │ WSL2 Bridge                      │
│                          ▼                                   │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  WSL2 Distro (Ubuntu/Alpine)                        │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  Exo Runtime (Linux binary)                     │  │   │
│  │  │  - Creates/manages containers                  │  │   │
│  │  │  - Namespace isolation                         │  │   │
│  │  │  - Cgroup v2 limits                           │  │   │
│  │  │  - Seccomp filtering                          │  │   │
│  │  │  - Daemonless, fast spawning                  │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  │                                                      │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  Agent Containers                             │  │   │
│  │  │  - OpenClaw agents                            │  │   │
│  │  │  - Python tools                               │  │   │
│  │  │  - Node.js tools                              │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### Why This Architecture Works

1. **Windows native orchestrator** - No cross-compilation needed
2. **WSL2 Linux runtime** - Full Linux container support
3. **Fast IPC** - WSL2 interop is fast (named pipes, Unix sockets)
4. **Agent-optimized** - Exo designed for AI workloads
5. **No Docker Desktop** - Removes 2GB+ daemon overhead

## Implementation Phases

### Phase 1: Basic WSL2 Bridge (MVP - 1-2 days)

**Goal**: Windows orchestrator can run containers via exo in WSL2

**Tasks**:
1. Build exo binary inside WSL2
   ```bash
   wsl
   cd /mnt/f/Software/exo
   cargo build --release
   ```

2. Create Windows wrapper script
   ```batch
   @echo off
   wsl /mnt/f/Software/exo/target/release/exo %*
   ```
   Save as: `F:\Software\exo\exo.cmd` (add to PATH)

3. Modify ExoClient to detect Windows
   ```rust
   // In container.rs ExoClient::new()
   #[cfg(windows)]
   let exo_path = exo_path.unwrap_or_else(|| {
       // Use wsl to call exo in WSL2
       "wsl /mnt/f/Software/exo/target/release/exo".to_string()
   });
   ```

4. Test basic operations
   - Create agent
   - Start agent
   - Stop agent
   - List containers

**Success Criteria**: Can create and run an OpenClaw agent using exo (not Docker)

---

### Phase 2: Volume Mounting (2-3 days)

**Goal**: Persistent data volumes work across WSL2 boundary

**Challenge**: Windows paths don't work inside WSL2 containers

**Solution**: WSL2 mount points
- Windows `F:\Software\...` → WSL2 `/mnt/f/Software/...`
- Or use WSL2 filesystem (9P) for better performance

**Tasks**:
1. Implement path translation in ExoClient
   ```rust
   fn translate_path_for_wsl(path: &str) -> String {
       if cfg!(windows) {
           // F:\Software\... → /mnt/f/Software/...
           path.replace('\\', "/")
               .replace("C:", "/mnt/c")
               .replace("F:", "/mnt/f")
               .replace("D:", "/mnt/d")
       } else {
           path.to_string()
       }
   }
   ```

2. Update volume mount logic in `setup_persistent_data_volume()`
3. Test agent data persistence

**Success Criteria**: Agent `/data` volume persists across restarts

---

### Phase 3: Performance Optimization (3-5 days)

**Goal**: Exo is faster than Docker for agent operations

**Current Docker Performance**:
- Container spawn: ~2-3 seconds
- Memory overhead: ~100MB per container
- Docker daemon: ~2GB RSS

**Target Exo Performance**:
- Container spawn: <500ms (daemonless!)
- Memory overhead: ~10-20MB per container
- No persistent daemon

**Tasks**:
1. **Persistent Exo Process in WSL2**
   - Don't spawn new `wsl exo` for each command
   - Run exo as daemon in WSL2
   - Use Unix socket or named pipe for IPC
   ```rust
   // Windows orchestrator → WSL2 exo daemon
   wsl --user root socat - UNIX-LISTEN:/exo.sock,fork EXEC:/mnt/f/Software/exo/target/release/exo
   ```

2. **Connection Pooling**
   - Reuse WSL2 connections
   - Keep WSL process alive
   - Batch operations

3. **Fast Spawning**
   - Exo is daemonless by design
   - No init system overhead
   - Direct process execution

**Performance Targets**:
- Agent create: <1s (vs Docker's ~3s)
- Agent start: <500ms (vs Docker's ~1s)
- Memory per agent: ~20MB (vs Docker's ~100MB)

---

### Phase 4: Agent-Specific Features (5-7 days)

**Goal**: Exo does things Docker can't for AI agents

**Why Exo is Better for Agents**:

1. **Stdio Communication**
   - Docker: Requires HTTP/WebSocket setup
   - Exo: Direct stdin/stdout (faster, simpler)
   ```
   # Docker
   docker run -p 18792:18792 agent-image
   # Then connect WebSocket to localhost:18792

   # Exo
   exo run --image python:3.12 --tool bash
   # Already talking via stdin/stdout
   ```

2. **Tool Bus**
   - Each tool gets isolated security context
   - Example: `bash` tool can't access network, `curl` can
   - Docker: All processes share container security context
   - Exo: Per-tool namespaces, seccomp filters

3. **Fast Iteration**
   - Exo: Spawn new container in <500ms
   - Docker: ~2-3 seconds
   - Critical for testing, A/B experimentation

4. **Resource Accounting**
   - Per-tool memory limits
   - Per-tool CPU quotas
   - Fine-grained tracking (not just per-container)

**Tasks**:
1. Implement tool-level sandboxing in Exo
2. Add agent protocol support (stdio-based)
3. Create agent-specific images
4. Document agent deployment patterns

**Success Criteria**: Deploying an agent is faster and more secure than Docker

---

### Phase 5: Windows Native Optimizations (Optional, 7-10 days)

**Goal**: Remove WSL2 overhead where possible

**Challenges**:
- Windows has no namespaces (need Windows Containers)
- Windows has no cgroups v2 (need Job Objects)
- Windows has no seccomp (need Windows Filter Platform)

**Possible Approaches**:

A. **Windows Containers + Hyper-V Isolation**
   - Uses Hyper-V for Linux kernel
   - Similar to Docker Desktop approach
   - Adds overhead (similar to Docker)

B. **Windows Subsystem for Linux (WSL2) - Recommended**
   - Full Linux compatibility
   - Fast interop (we already use this)
   - GPU passthrough works
   - Minimal overhead

C. **Pure Windows Runtime (Long-term)**
   - Reimplement namespace concepts using Windows Jobs
   - Use Windows Container isolation
   - **Huge effort, questionable value**

**Recommendation**: Stick with WSL2 approach (Phase 1-4), optimize the bridge

---

## What Needs to Be Built

### Immediate (This Week)

1. **WSL2 Exo Build Script**
   - Automate building exo in WSL2
   - Create Windows wrapper
   - Test orchestrator integration

2. **Path Translation Layer**
   - Windows paths → WSL2 paths
   - Volume mounting support
   - Test with `/data` volumes

3. **Performance Baseline**
   - Measure Docker vs Exo spawn times
   - Memory usage comparison
   - Document improvements

### Short-term (This Month)

4. **Persistent WSL2 Connection**
   - Exo daemon in WSL2
   - Unix socket IPC
   - Connection pooling

5. **Agent Protocol Support**
   - Stdio-based communication
   - Tool bus implementation
   - Security contexts per tool

6. **Testing Suite**
   - Compare Docker vs Exo
   - Performance benchmarks
   - Memory profiling

### Long-term (Next Quarter)

7. **Advanced Features**
   - GPU passthrough optimization
   - Custom agent images
   - Snapshot/restore
   - Live migration

8. **Production Hardening**
   - Error handling
   - Recovery mechanisms
   - Monitoring
   - Logging

---

## Performance Targets

| Metric | Docker (Current) | Exo (Target) | Improvement |
|--------|-------------------|---------------|-------------|
| Agent spawn | 2-3s | <500ms | **4-6x faster** |
| Agent start | 1-2s | <200ms | **5-10x faster** |
| Memory/agent | ~100MB | ~20MB | **5x reduction** |
| Daemon overhead | ~2GB RSS | 0MB | **100% reduction** |
| Tool isolation | Container-level | Tool-level | **Fine-grained** |

---

## Risks & Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| WSL2 overhead | Slower than native Linux | Use persistent connection, batch operations |
| Path translation bugs | Data loss | Comprehensive testing, validation |
| WSL2 version issues | Compatibility | Pin WSL2 kernel version, test matrix |
| Windows Update breaks WSL2 | Downtime | Document recovery procedures |
| Exo bugs in production | Instability | Keep Docker fallback, gradual migration |

---

## Success Criteria

### Minimum Viable Product (MVP)
- ✅ Can create agent with exo (not Docker)
- ✅ Agent can start/stop/reliably
- ✅ Persistent volumes work
- ✅ Performance equal or better than Docker

### Production Ready
- ✅ All features work (volumes, ports, env vars)
- ✅ Error handling robust
- ✅ Monitoring/logging in place
- ✅ Migration path from Docker

### Agent-Optimized
- ✅ Stdio communication (not HTTP)
- ✅ Tool bus with per-tool security
- ✅ Fast spawning (<500ms)
- ✅ Low overhead (<20MB per agent)

---

## Next Steps for Developer

1. **Read this document** (done!)
2. **Start Phase 1**: Build exo in WSL2
3. **Test orchestrator integration**: Modify ExoClient for WSL2
4. **Document findings**: Update this roadmap with real measurements

**Estimated timeline to MVP**: 3-5 days of focused work

**Estimated timeline to production**: 2-3 weeks

---

## Questions to Answer

1. Should we support both Docker and Exo in parallel? (Yes, for migration)
2. Do we need GUI for WSL2 management? (Maybe later)
3. Should agents be able to choose runtime? (Already supported!)
4. What's the rollback plan if Exo has issues? (Keep Docker)

---

## Conclusion

**Replacing Docker with Exo on Windows is feasible** using the WSL2 bridge approach.

**Key advantages**:
- 4-6x faster agent spawning
- 5x lower memory usage
- Agent-specific features (tool bus, stdio protocol)
- No Docker Desktop dependency

**Recommended path**: Phases 1-4 over 2-3 weeks

**End state**: Fast, lightweight agent runtime optimized for AI workloads
