#!/bin/bash
# Build exo-agent container images
#
# Usage:
#   ./build.sh           # Build standard image
#   ./build.sh slim      # Build slim image
#   ./build.sh all       # Build both
#
# Options via environment:
#   CONTAINER_TOOL=docker|podman   # default: docker if available, else podman
#   VERSION=0.1.0                  # image version tag

set -e

cd "$(dirname "$0")/../.."

TAG="${1:-standard}"
VERSION="${VERSION:-0.1.0}"
if [ -n "${CONTAINER_TOOL:-}" ]; then
    ENGINE="$CONTAINER_TOOL"
elif command -v docker >/dev/null 2>&1; then
    ENGINE="docker"
elif command -v podman >/dev/null 2>&1; then
    ENGINE="podman"
else
    echo "Neither docker nor podman was found on PATH." >&2
    exit 127
fi

echo "Building exo-agent container ($TAG) with $ENGINE..."

case "$TAG" in
    standard|"")
        "$ENGINE" build \
            -t exo-agent:$VERSION \
            -t exo-agent:latest \
            -f images/exo-agent/Containerfile \
            .
        ;;
    slim)
        "$ENGINE" build \
            -t exo-agent:$VERSION-slim \
            -t exo-agent:slim \
            -f images/exo-agent/Containerfile.slim \
            .
        ;;
    all)
        $0 standard
        $0 slim
        ;;
    *)
        echo "Unknown variant: $TAG"
        echo "Usage: $0 [standard|slim|all]"
        exit 1
        ;;
esac

echo "Done! Image size:"
"$ENGINE" images exo-agent --format "{{.Repository}}:{{.Tag}}\t{{.Size}}"
