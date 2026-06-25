#!/usr/bin/env bash
# Install the `exo` binary to ~/.local/bin (or /usr/local/bin with --system).
# Usage: curl -sSL https://get.exo.dev | bash
#        ./scripts/install.sh --version 0.1.0 --prefix ~/.local

set -euo pipefail

VERSION="${EXO_VERSION:-latest}"
PREFIX="${EXO_PREFIX:-$HOME/.local}"
SYSTEM=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --prefix)
            PREFIX="$2"
            shift 2
            ;;
        --system)
            SYSTEM=true
            PREFIX="/usr/local"
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64) ARCH="x86_64" ;;
    amd64) ARCH="x86_64" ;;
    arm64) ARCH="aarch64" ;;
    aarch64) ARCH="aarch64" ;;
    *)
        echo "Unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

case "$OS" in
    linux) TARGET="${ARCH}-unknown-linux-gnu" ;;
    darwin) TARGET="${ARCH}-apple-darwin" ;;
    mingw*|msys*|cygwin*|windows)
        TARGET="x86_64-pc-windows-msvc"
        EXE=".exe"
        ;;
    *)
        echo "Unsupported OS: $OS" >&2
        exit 1
        ;;
esac

if [[ "$VERSION" == "latest" ]]; then
    # In a real release pipeline this resolves the newest GitHub release tag.
    VERSION="0.1.0"
fi

BIN="exo${EXE:-}"
ARTIFACT="exo-${VERSION}-${TARGET}${EXE:-}"
URL="https://github.com/clawpen/exo/releases/download/v${VERSION}/${ARTIFACT}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading $URL..."
if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$URL" -o "$TMP_DIR/$BIN"
elif command -v wget >/dev/null 2>&1; then
    wget -q "$URL" -O "$TMP_DIR/$BIN"
else
    echo "curl or wget is required" >&2
    exit 1
fi

chmod +x "$TMP_DIR/$BIN"

INSTALL_DIR="$PREFIX/bin"
mkdir -p "$INSTALL_DIR"
cp "$TMP_DIR/$BIN" "$INSTALL_DIR/$BIN"

echo "Installed $BIN to $INSTALL_DIR/$BIN"

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo "Add $INSTALL_DIR to your PATH to use exo:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi
