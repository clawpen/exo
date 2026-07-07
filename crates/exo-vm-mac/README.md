# exo-vm-mac

Exo-managed Linux microVM backend for macOS, built on Apple Virtualization.framework.

## Scope

This crate is a boot prototype. It can:

- Download and cache a minimal Alpine Linux kernel/initrd.
- Build an Exo-patched initramfs containing a tiny guest agent.
- Boot a Linux VM with `VZLinuxBootLoader`.
- Communicate with the guest agent over `VZVirtioSocket` (vsock).
- Provide `exo vm init/start/status/stop/reset` commands.

It does not yet run OCI containers, manage overlayfs, or provide container networking. Those are future phases.

## Building the guest agent

The guest agent is a Rust binary that must run inside the Linux VM. For Apple Silicon Macs it should be built for `aarch64-unknown-linux-musl`:

```bash
rustup target add aarch64-unknown-linux-musl
# Install a musl cross toolchain, e.g.:
# brew install messense/macos-cross-toolchains/aarch64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl -p exo-vm-mac-guest
exo vm install-guest-agent target/aarch64-unknown-linux-musl/release/exo-vm-guest-init
exo vm init --force
```

For the first prototype, if a prebuilt guest agent is not available, `exo vm init` falls back to a minimal shell-based init that proves the VM boots.

## Usage

```bash
exo vm init
exo vm start
exo vm status
exo vm stop
exo vm reset
```

## Testing

Full boot tests require a macOS host with Virtualization.framework support (Apple Silicon, macOS 12+). GitHub-hosted `macos-latest` runners cannot boot VMs, so runtime tests are gated behind `EXO_VM_TEST_RUN=1` and should be run on a local Mac or self-hosted runner.
