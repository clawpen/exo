#!/bin/bash
# bench-openclaw.sh — Compare OpenClaw agent startup in Docker vs Exo.
#
# Pulls the published OpenClaw agent image, imports it into Exo, then measures
# the wall-clock time to run `openclaw --version` in each runtime.
#
# Run inside WSL2/Linux with `exo` and `docker` on PATH (or set EXO_BIN / DOCKER_BIN).
# Output: human table + machine-readable results/bench-openclaw-YYYYMMDD-HHMMSS.json
set -euo pipefail

EXO_BIN="${EXO_BIN:-$(command -v exo || echo /mnt/f/Software/exo/target/release/exo)}"
DOCKER_BIN="${DOCKER_BIN:-$(command -v docker || true)}"
OUT_DIR="${OUT_DIR:-$(dirname "$0")/../results}"
IMAGE="${OPENCLAW_IMAGE:-ghcr.io/clawpen/openclaw-agent:latest}"
RUNS="${RUNS:-5}"
STAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT_DIR"
JSON="$OUT_DIR/bench-openclaw-$STAMP.json"

note(){ printf '\033[0;36m%s\033[0m\n' "$*"; }
ok(){ printf '\033[0;32m%s\033[0m\n' "$*"; }
warn(){ printf '\033[1;33m%s\033[0m\n' "$*"; }

have_docker(){ [ -n "$DOCKER_BIN" ] && "$DOCKER_BIN" info >/dev/null 2>&1; }

# --- median of RUNS timings (ms) ---------------------------------------------
median_ms(){
  local -a t=()
  for _ in $(seq 1 "$RUNS"); do
    local s e; s=$(date +%s%3N); eval "$1" >/dev/null 2>&1 || true; e=$(date +%s%3N)
    t+=($((e - s)))
  done
  printf '%s\n' "${t[@]}" | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'
}

# --- ensure image in both runtimes -------------------------------------------
ensure_images(){
  note "== Ensuring OpenClaw agent image in both runtimes =="

  if ! have_docker; then
    warn "Docker not available; cannot obtain OpenClaw image"
    return 1
  fi

  note "  Pulling $IMAGE with Docker..."
  "$DOCKER_BIN" pull "$IMAGE" >/dev/null 2>&1 || {
    warn "Failed to pull $IMAGE"
    return 1
  }

  note "  Exporting image for Exo import..."
  TMP_TAR="$(mktemp --suffix=.tar.gz)"
  trap 'rm -f "$TMP_TAR"' EXIT
  "$DOCKER_BIN" save "$IMAGE" | gzip > "$TMP_TAR"

  note "  Importing $IMAGE into Exo..."
  "$EXO_BIN" import "$TMP_TAR" >/dev/null 2>&1 || {
    warn "Failed to import $IMAGE into Exo"
    return 1
  }

  ok "  Image ready in both runtimes"
}

# --- benchmark ---------------------------------------------------------------
run_benchmark(){
  local cmd_docker="$DOCKER_BIN run --rm $IMAGE node /usr/local/lib/node_modules/openclaw/dist/cli.js --version"
  local cmd_exo="$EXO_BIN run --rm $IMAGE -- node /usr/local/lib/node_modules/openclaw/dist/cli.js --version"

  note "== OpenClaw agent --version (median of $RUNS) =="

  exo_ms=$(median_ms "$cmd_exo")
  ok "  Exo:    ${exo_ms} ms"

  docker_ms=0
  if have_docker; then
    docker_ms=$(median_ms "$cmd_docker")
    ok "  Docker: ${docker_ms} ms"
  else
    warn "  Docker not available — skipping Docker number"
  fi

  pct(){ [ "$2" -gt 0 ] && echo "$(( (($2 - $1) * 100) / $2 ))" || echo "n/a"; }

  note "== Summary =="
  printf '  %-22s %s%%\n' "OpenClaw startup faster:" "$(pct "$exo_ms" "$docker_ms")"

  cat > "$JSON" <<EOF
{
  "stamp": "$STAMP",
  "image": "$IMAGE",
  "runs": $RUNS,
  "command": "openclaw --version",
  "startup_ms": {
    "exo": $exo_ms,
    "docker": $docker_ms
  }
}
EOF
  ok "Wrote $JSON"
}

# --- main --------------------------------------------------------------------
if ! [ -x "$EXO_BIN" ]; then
  warn "Exo binary not found at $EXO_BIN"
  exit 1
fi

ensure_images
run_benchmark
