#!/usr/bin/env bash
set -euo pipefail

variant="${1:-standard}"
tag="${TAG:-exo-agent:docker-test}"

cd "$(dirname "$0")/.."

docker info >/dev/null

case "$variant" in
  standard) dockerfile="images/exo-agent/Containerfile" ;;
  slim) dockerfile="images/exo-agent/Containerfile.slim" ;;
  *)
    echo "Usage: $0 [standard|slim]" >&2
    exit 2
    ;;
esac

docker build -t "$tag" -f "$dockerfile" .

echo "==> CLI help smoke test"
docker run --rm "$tag" --help

echo "==> EOF/stdin lifecycle smoke test"
# With stdin closed, the agent initializes config + SQLite memory, observes EOF,
# and exits without making an LLM API call.
docker run --rm "$tag"

echo "Docker smoke tests passed for $tag ($variant)."
