# Exo Container Runtime - Development Roadmap

## Vision

A production-ready, agent-first container runtime that replaces Docker for Claw Pen deployments.

## Current State (2026-03-09)

✅ Working:
- Container isolation (namespaces, seccomp, capabilities)
- Alpine rootfs extraction
- Basic `run --rm` command
- Rootless by default

⚠️ In Progress (agents running):
- Persistent containers (stop/start/remove)
- Claw Pen integration

## Phase 1: Core Functionality (Week 1)

### 1.1 Persistent Containers [IN PROGRESS]
- [ ] `--name` flag for named containers
- [ ] `exo stop <name>`
- [ ] `exo start <name>`
- [ ] `exo remove <name>`
- [ ] `exo list --json`
- [ ] Container metadata storage

### 1.2 Overlayfs Writable Layer
- [ ] Create overlay mount with lowerdir (image) + upperdir (writable)
- [ ] Persist changes across container restarts
- [ ] Clean upperdir on container remove
- [ ] Handle overlay mount in user namespace

### 1.3 Volume Mounts
- [ ] `-v /host/path:/container/path` syntax
- [ ] Named volumes (`-v data:/app/data`)
- [ ] Proper permission handling (uid/gid mapping)
- [ ] Volume isolation between containers

### 1.4 TTY/Exec Mode
- [ ] `exo exec -it <name> <command>`
- [ ] PTY allocation
- [ ] Stdin forwarding
- [ ] Signal handling (Ctrl+C)

## Phase 2: Image Support (Week 2)

### 2.1 OCI Registry Pull
- [ ] Docker Hub authentication
- [ ] Manifest parsing
- [ ] Layer download
- [ ] Layer extraction
- [ ] Image caching

### 2.2 Layer Management
- [ ] Content-addressable storage
- [ ] Layer deduplication
- [ ] `exo images` command
- [ ] `exo rmi` command
- [ ] Prune unused layers

### 2.3 Image Building
- [ ] Dockerfile parsing (basic)
- [ ] `exo build -t name .`
- [ ] Layer commit
- [ ] Build caching

## Phase 3: Networking (Week 3)

### 3.1 Network Namespace
- [ ] Create isolated network namespace
- [ ] Virtual ethernet pair (veth)
- [ ] Bridge network (exo0)
- [ ] IP allocation

### 3.2 Port Forwarding
- [ ] `-p host:container` syntax
- [ ] Port mapping via iptables/nftables
- [ ] Multiple port mappings

### 3.3 DNS
- [ ] Container DNS resolution
- [ ] Custom DNS servers
- [ ] /etc/hosts management

## Phase 4: Resource Management (Week 4)

### 4.1 Cgroup v2 Support
- [ ] Cgroup delegation for rootless
- [ ] Memory limits
- [ ] CPU limits
- [ ] I/O limits

### 4.2 Resource Monitoring
- [ ] `exo stats <name>`
- [ ] Real-time metrics
- [ ] Historical data

## Phase 5: Agent Features (Week 5)

### 5.1 Agent Communication Protocol
- [ ] Structured JSON messaging over stdio
- [ ] Tool request/response format
- [ ] Tool bus for sandboxed execution
- [ ] Capability negotiation

### 5.2 Health Checks
- [ ] `--health-cmd` option
- [ ] Periodic health checks
- [ ] Auto-restart on failure
- [ ] Health status in `exo list`

### 5.3 Secrets Management
- [ ] `--secret name=value` option
- [ ] Secrets file mounting
- [ ] Environment injection
- [ ] Secret rotation

## Phase 6: Production (Week 6)

### 6.1 Logging
- [ ] Structured logging
- [ ] Log rotation
- [ ] Log aggregation support

### 6.2 Security Hardening
- [ ] Security audit
- [ ] Fuzzing
- [ ] SELinux/AppArmor profiles

### 6.3 Documentation
- [ ] User guide
- [ ] API reference
- [ ] Architecture docs
- [ ] Migration guide (from Docker)

## Tracking

- Started: 2026-03-09
- Target: 2026-04-20 (6 weeks)
- Current Phase: 1.1 (Persistent Containers)

## Notes

- Each phase should be usable independently
- Maintain backward compatibility with CLI
- Test on both Linux and WSL2
- Keep rootless as default
