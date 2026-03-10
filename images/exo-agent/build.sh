#!/bin/bash
# Build exo-agent container images
#
# Usage:
#   ./build.sh           # Build standard image
#   ./build.sh slim      # Build ultra-slim image
#   ./build.sh all       # Build both

set -e

cd "$(dirname "$0")/../.."

TAG="${1:-standard}"
VERSION="${VERSION:-0.1.0}"

echo "Building exo-agent container ($TAG)..."

case "$TAG" in
    standard|"")
        podman build \
            -t exo-agent:$VERSION \
            -t exo-agent:latest \
            -f images/exo-agent/Containerfile \
            .
        ;;
    slim)
        podman build \
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
podman images exo-agent --format "{{.Repository}}:{{.Tag}}\t{{.Size}}"
