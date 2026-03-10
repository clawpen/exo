#!/bin/bash
# Build OpenClaw agent image for exo

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE_NAME="openclaw-agent"
IMAGE_TAG="latest"

echo "Building ${IMAGE_NAME}:${IMAGE_TAG}..."

# Check for Docker
if ! command -v docker &> /dev/null; then
    echo "Error: Docker is required to build images"
    echo "Install Docker or use the pre-built image from ghcr.io/clawpen/openclaw-agent"
    exit 1
fi

# Create openclaw bundle directory
BUNDLE_DIR="${SCRIPT_DIR}/openclaw"
mkdir -p "${BUNDLE_DIR}"

# Bundle OpenClaw from installed version
OPENCLAW_SRC="${OPENCLAW_SRC:-$HOME/.nvm/versions/node/v24.13.1/lib/node_modules/openclaw}"

if [ -d "${OPENCLAW_SRC}" ]; then
    echo "Bundling OpenClaw from ${OPENCLAW_SRC}..."
    cp -r "${OPENCLAW_SRC}/" "${BUNDLE_DIR}/"
else
    echo "Error: OpenClaw not found at ${OPENCLAW_SRC}"
    echo "Set OPENCLAW_SRC to the OpenClaw installation directory"
    exit 1
fi

# Copy workspace files
WORKSPACE_SRC="${SCRIPT_DIR}/workspace"
mkdir -p "${WORKSPACE_SRC}"
cp -r "${SCRIPT_DIR}/../../templates/workspace/"* "${WORKSPACE_SRC}/" 2>/dev/null || true

# Build with Docker
cd "${SCRIPT_DIR}"
docker build -t "${IMAGE_NAME}:${IMAGE_TAG}" -f Containerfile .

# Export for exo
echo "Exporting image..."
docker save "${IMAGE_NAME}:${IMAGE_TAG}" | gzip > "${SCRIPT_DIR}/${IMAGE_NAME}-${IMAGE_TAG}.tar.gz"

echo "✅ Image built: ${IMAGE_NAME}:${IMAGE_TAG}"
echo "   Exported to: ${SCRIPT_DIR}/${IMAGE_NAME}-${IMAGE_TAG}.tar.gz"
echo ""
echo "To use with exo (once import is supported):"
echo "  exo import ${IMAGE_NAME}-${IMAGE_TAG}.tar.gz"
