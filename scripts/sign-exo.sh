#!/bin/bash
# Sign the local exo binary with the Virtualization.framework entitlement.
# This must be done after every cargo build that replaces target/debug/exo or
# target/release/exo; otherwise VZVirtualMachine.canStart stays false on macOS.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$SCRIPT_DIR/.."
ENTITLEMENTS="$REPO_ROOT/crates/exo-vm-mac/entitlements.plist"

PROFILE="${1:-debug}"
case "$PROFILE" in
    debug)   BINARY="$REPO_ROOT/target/debug/exo";;
    release) BINARY="$REPO_ROOT/target/release/exo";;
    *)       echo "Usage: $0 [debug|release]"; exit 1;;
esac

if [[ ! -f "$ENTITLEMENTS" ]]; then
    echo "Entitlements file not found: $ENTITLEMENTS" >&2
    exit 1
fi

if [[ ! -f "$BINARY" ]]; then
    echo "exo binary not found: $BINARY" >&2
    exit 1
fi

echo "Signing $BINARY with $ENTITLEMENTS ..."
codesign --sign - --force --entitlements "$ENTITLEMENTS" "$BINARY"
echo "Done. Verifying entitlement:"
codesign -dv --entitlements - "$BINARY" 2>&1 | grep -A1 "com.apple.security.virtualization"
