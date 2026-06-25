#!/usr/bin/env bash
# Build release binaries for Linux, macOS, and Windows.
# Usage: scripts/build-release.sh [version]
# Outputs to target/release-artifacts/

set -euo pipefail

VERSION="${1:-$(git describe --tags --always --dirty)}"
ARTIFACT_DIR="target/release-artifacts"

echo "Building Exo release binaries (version: $VERSION)"

mkdir -p "$ARTIFACT_DIR"

build_target() {
    local target="$1"
    local ext="$2"
    local bin_name="exo${ext}"

    echo "Building for $target..."
    cargo build --release --target "$target" -p exo

    local src="target/$target/release/$bin_name"
    local dst="$ARTIFACT_DIR/exo-${VERSION}-${target}${ext}"

    if [[ -f "$src" ]]; then
        cp "$src" "$dst"
        echo "  -> $dst"
    else
        echo "  build for $target did not produce $src" >&2
        return 1
    fi
}

# Native target first (always works on the host OS).
native_target="$(rustc -vV | awk '/host/{print $2}')"
build_target "$native_target" ""

# Cross targets if toolchains are installed.
for target in x86_64-unknown-linux-gnu x86_64-unknown-linux-musl aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc; do
    if rustup target list --installed | grep -q "^${target}\b"; then
        case "$target" in
            *windows*) build_target "$target" ".exe" ;;
            *) build_target "$target" "" ;;
        esac
    else
        echo "Skipping $target (toolchain not installed)"
    fi
done

echo "Release artifacts in $ARTIFACT_DIR:"
ls -la "$ARTIFACT_DIR"
