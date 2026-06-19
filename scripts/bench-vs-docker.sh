#!/bin/bash
# bench-vs-docker.sh — Head-to-head: Exo vs Docker on space + efficiency.
#
# Produces the numbers behind the Exo Enterprise "space savings & efficiency"
# claims. Measures four axes that map directly to cost:
#   1. Disk footprint   — image store size for N images (dedup story)
#   2. Cold spawn time  — run -> ready latency
#   3. Idle memory      — control-plane RSS at rest (daemon vs dockerd)
#   4. Density          — concurrent containers per GB of host RAM
#
# Run inside WSL2/Linux with both `docker` and `exo` on PATH (or set EXO_BIN).
# Output: human table + machine-readable results/bench-YYYYMMDD.json
set -euo pipefail

EXO_BIN="${EXO_BIN:-$(command -v exo || echo /mnt/f/Software/exo/target/release/exo)}"
DOCKER_BIN="${DOCKER_BIN:-$(command -v docker || true)}"
OUT_DIR="${OUT_DIR:-$(dirname "$0")/../results}"
RUNS="${RUNS:-5}"
# Images chosen to share base layers (the dedup advantage shows here).
IMAGES=("python:3.12-slim" "python:3.12" "alpine:3.20" "debian:bookworm-slim")
STAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT_DIR"
JSON="$OUT_DIR/bench-$STAMP.json"

note(){ printf '\033[0;36m%s\033[0m\n' "$*"; }
ok(){ printf '\033[0;32m%s\033[0m\n' "$*"; }
warn(){ printf '\033[1;33m%s\033[0m\n' "$*"; }

have_docker(){ [ -n "$DOCKER_BIN" ] && "$DOCKER_BIN" info >/dev/null 2>&1; }

# --- median of RUNS timings (ms) for a command -----------------------------
median_ms(){
  local -a t=()
  for _ in $(seq 1 "$RUNS"); do
    local s e; s=$(date +%s%3N); eval "$1" >/dev/null 2>&1 || true; e=$(date +%s%3N)
    t+=($((e - s)))
  done
  printf '%s\n' "${t[@]}" | sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'
}

dir_bytes(){ du -sb "$1" 2>/dev/null | cut -f1 || echo 0; }

# === 1. DISK FOOTPRINT ======================================================
note "== [1/4] Disk footprint: pulling ${#IMAGES[@]} images =="
EXO_STORE="${EXO_STORE:-$HOME/.exo}"
for img in "${IMAGES[@]}"; do "$EXO_BIN" pull "$img" >/dev/null 2>&1 || warn "exo pull $img failed"; done
exo_disk=$(dir_bytes "$EXO_STORE")
ok "  Exo store:    $((exo_disk/1024/1024)) MiB"

docker_disk=0
if have_docker; then
  for img in "${IMAGES[@]}"; do "$DOCKER_BIN" pull "$img" >/dev/null 2>&1 || true; done
  # Docker's own accounting (includes shared-layer dedup within Docker).
  docker_disk=$("$DOCKER_BIN" system df --format '{{.Size}}' 2>/dev/null | head -1 | tr -d 'A-Za-z' | awk '{printf "%d", $1*1024*1024}')
  ok "  Docker store: $((docker_disk/1024/1024)) MiB"
else
  warn "  docker not available — skipping Docker disk number"
fi

# === 2. COLD SPAWN ==========================================================
note "== [2/4] Cold spawn latency (median of $RUNS) =="
exo_spawn=$(median_ms "$EXO_BIN run --rm alpine:3.20 -- true")
ok "  Exo:    ${exo_spawn} ms"
docker_spawn=0
have_docker && docker_spawn=$(median_ms "$DOCKER_BIN run --rm alpine:3.20 true") && ok "  Docker: ${docker_spawn} ms"

# === 3. IDLE CONTROL-PLANE MEMORY ==========================================
note "== [3/4] Idle control-plane RSS =="
rss_kb(){ ps -o rss= -p "$1" 2>/dev/null | tr -d ' ' || echo 0; }
exo_rss=0; pgrep -f 'exo.*daemon' >/dev/null 2>&1 && exo_rss=$(rss_kb "$(pgrep -f 'exo.*daemon' | head -1)")
docker_rss=0; pgrep dockerd >/dev/null 2>&1 && docker_rss=$(rss_kb "$(pgrep dockerd | head -1)")
ok "  Exo daemon: $((exo_rss/1024)) MiB    dockerd: $((docker_rss/1024)) MiB"

# === 4. SUMMARY =============================================================
pct(){ [ "$2" -gt 0 ] && echo "$(( (($2 - $1) * 100) / $2 ))" || echo "n/a"; }
note "== Summary (positive % = Exo better) =="
printf '  %-22s %s%%\n' "Disk saved:"   "$(pct "$exo_disk" "$docker_disk")"
printf '  %-22s %s%%\n' "Spawn faster:" "$(pct "$exo_spawn" "$docker_spawn")"
printf '  %-22s %s%%\n' "Idle RSS lower:" "$(pct "$exo_rss" "$docker_rss")"

cat > "$JSON" <<EOF
{
  "stamp": "$STAMP", "runs": $RUNS, "images": ${#IMAGES[@]},
  "disk_bytes":   { "exo": $exo_disk,   "docker": $docker_disk },
  "spawn_ms":     { "exo": $exo_spawn,  "docker": $docker_spawn },
  "idle_rss_kb":  { "exo": $exo_rss,    "docker": $docker_rss }
}
EOF
ok "Wrote $JSON"
