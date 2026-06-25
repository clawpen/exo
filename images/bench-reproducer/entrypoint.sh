#!/bin/bash
# entrypoint.sh — Run the Exo benchmark suite inside the reproducer container.
set -euo pipefail

OUT_DIR="${OUT_DIR:-/results}"
mkdir -p "$OUT_DIR"

echo "== Exo benchmark reproducer =="
echo "Exo binary: $EXO_BIN"
echo "Docker binary: $DOCKER_BIN"
echo "Output: $OUT_DIR"
echo ""

# Verify Docker socket access.
if ! "$DOCKER_BIN" info > /dev/null 2>&1; then
  echo "Warning: Docker socket not accessible; Docker-side numbers will be skipped."
fi

# Run head-to-head benchmark.
echo "== Running bench-vs-docker.sh =="
/usr/local/bin/bench-vs-docker.sh

# Run density benchmark.
echo "== Running bench-density.sh =="
/usr/local/bin/bench-density.sh

# Run OpenClaw agent benchmark.
echo "== Running bench-openclaw.sh =="
/usr/local/bin/bench-openclaw.sh || true

# Generate Markdown summary if PowerShell is available or fallback to jq listing.
echo "== Generating summary =="
if command -v pwsh > /dev/null 2>&1; then
  pwsh -ExecutionPolicy Bypass -File /usr/local/bin/bench-to-markdown.ps1 \
    -JsonPath "$OUT_DIR"/bench-*.json -OutFile "$OUT_DIR/summary.md"
  echo "Wrote $OUT_DIR/summary.md"
elif command -v powershell > /dev/null 2>&1; then
  powershell -ExecutionPolicy Bypass -File /usr/local/bin/bench-to-markdown.ps1 \
    -JsonPath "$OUT_DIR"/bench-*.json -OutFile "$OUT_DIR/summary.md"
  echo "Wrote $OUT_DIR/summary.md"
else
  echo "PowerShell not available; listing JSON files:"
  ls -la "$OUT_DIR"/bench-*.json
fi

echo ""
echo "Done. Results are in $OUT_DIR."
