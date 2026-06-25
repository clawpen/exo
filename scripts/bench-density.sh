#!/bin/bash
# bench-density.sh — Ramp concurrent containers and measure memory density.
#
# Produces the "concurrent containers per GB RAM" number behind the Exo
# Enterprise density claim. Creates detached containers in batches, records
# per-container RSS, and stops at the first failure.
#
# Run inside WSL2/Linux with `exo` on PATH (or set EXO_BIN).
# Output: human table + machine-readable results/bench-density-YYYYMMDD-HHMMSS.json
set -euo pipefail

EXO_BIN="${EXO_BIN:-$(command -v exo || echo /mnt/f/Software/exo/target/release/exo)}"
DOCKER_BIN="${DOCKER_BIN:-$(command -v docker || true)}"
OUT_DIR="${OUT_DIR:-$(dirname "$0")/../results}"
IMAGE="${IMAGE:-alpine:3.20}"
COMMAND="${COMMAND:-sleep 30}"
BATCH="${BATCH:-10}"
MAX="${MAX:-1000}"
WAIT_READY="${WAIT_READY:-2}"
STAMP="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT_DIR"
JSON="$OUT_DIR/bench-density-$STAMP.json"

note(){ printf '\033[0;36m%s\033[0m\n' "$*"; }
ok(){ printf '\033[0;32m%s\033[0m\n' "$*"; }
warn(){ printf '\033[1;33m%s\033[0m\n' "$*"; }

have_docker(){ [ -n "$DOCKER_BIN" ] && "$DOCKER_BIN" info >/dev/null 2>&1; }

# --- start / reuse exo daemon ------------------------------------------------
start_exo_daemon(){
  if [ -S /tmp/exo-daemon.sock ]; then
    if "$EXO_BIN" daemon --status >/dev/null 2>&1; then
      note "Reusing running exo daemon"
      return 0
    fi
    warn "Stale exo daemon socket found; removing"
    rm -f /tmp/exo-daemon.sock
  fi

  note "Starting exo daemon for density benchmark"
  nohup "$EXO_BIN" daemon --foreground >/tmp/exo-density-daemon.log 2>&1 &
  local pid=$!

  # Wait for socket (up to 10s)
  for _ in $(seq 1 50); do
    if [ -S /tmp/exo-daemon.sock ]; then
      ok "Exo daemon ready (PID: $pid)"
      return 0
    fi
    sleep 0.2
  done
  warn "Daemon did not start; see /tmp/exo-density-daemon.log"
  return 1
}

stop_exo_daemon(){
  if [ "${STOP_DAEMON:-0}" = "1" ]; then
    note "Stopping exo daemon"
    "$EXO_BIN" daemon --stop >/dev/null 2>&1 || true
  fi
}

# --- helpers -----------------------------------------------------------------
cgroup_memory_bytes(){
  local name=$1
  local path="/sys/fs/cgroup/exo/$name/memory.current"
  if [ -r "$path" ]; then
    cat "$path" 2>/dev/null || echo 0
  else
    echo 0
  fi
}

host_total_rss_kb(){
  awk '/^MemAvailable:/ {print $2} /(^MemTotal:|^MemFree:|^Buffers:|^Cached:|^SReclaimable:)/ {sum+=$2} END {print sum}' /proc/meminfo 2>/dev/null || echo 0
}

# --- density ramp ------------------------------------------------------------
note "== Density ramp: $IMAGE, batch size $BATCH, max $MAX =="
"$EXO_BIN" pull "$IMAGE" >/dev/null 2>&1 || warn "exo pull $IMAGE failed or image already present"

start_exo_daemon

SAMPLES_JSON=""
MAX_CONTAINERS=0
FAILURE_POINT=0
FAILURE_REASON=""
TOTAL_RSS_AT_MAX=0
PER_CONTAINER_AT_MAX=0
HOST_RSS_AT_MAX=0

for n in $(seq "$BATCH" "$BATCH" "$MAX"); do
  note "  Starting batch up to $n containers..."
  failed=0
  for i in $(seq 1 "$BATCH"); do
    idx=$((n - BATCH + i))
    name="exo-density-$idx"
    if ! "$EXO_BIN" run -d --name "$name" "$IMAGE" -- $COMMAND >/dev/null 2>&1; then
      failed=$i
      break 2
    fi
  done

  sleep "$WAIT_READY"

  # Measure RSS for the newest batch (last container)
  last_name="exo-density-$n"
  last_rss=$(cgroup_memory_bytes "$last_name")

  # Total RSS across all running density containers
  total_rss=0
  for j in $(seq 1 "$n"); do
    rss=$(cgroup_memory_bytes "exo-density-$j")
    total_rss=$((total_rss + rss))
  done

  host_rss=$(host_total_rss_kb)
  host_rss_mb=$((host_rss / 1024))
  per_container=$((total_rss / n))
  MAX_CONTAINERS=$n
  TOTAL_RSS_AT_MAX=$total_rss
  PER_CONTAINER_AT_MAX=$per_container
  HOST_RSS_AT_MAX=$host_rss

  [ -n "$SAMPLES_JSON" ] && SAMPLES_JSON="$SAMPLES_JSON,"
  SAMPLES_JSON="$SAMPLES_JSON{\"n\": $n, \"total_rss_kb\": $((total_rss / 1024)), \"per_container_kb\": $((per_container / 1024)), \"host_used_rss_mb\": $host_rss_mb}"

  printf '    %4d containers | total RSS %5d MiB | per-container %5d KiB | host used %5d MiB\n' \
    "$n" "$((total_rss / 1024 / 1024))" "$((per_container / 1024))" "$host_rss_mb"
done

if [ "$failed" -gt 0 ]; then
  FAILURE_POINT=$((MAX_CONTAINERS + failed))
  FAILURE_REASON="container $FAILURE_POINT failed to start"
  warn "  Stopped at $FAILURE_POINT ($FAILURE_REASON)"
else
  note "  Reached configured max of $MAX containers"
fi

# --- optional Docker density comparison --------------------------------------
DOCKER_MAX=0
DOCKER_PER_CONTAINER=0
if have_docker; then
  note "== Optional Docker density comparison =="
  "$DOCKER_BIN" pull "$IMAGE" >/dev/null 2>&1 || true
  for n in $(seq "$BATCH" "$BATCH" "$MAX"); do
    d_failed=0
    for i in $(seq 1 "$BATCH"); do
      idx=$((n - BATCH + i))
      if ! "$DOCKER_BIN" run -d --name "docker-density-$idx" "$IMAGE" $COMMAND >/dev/null 2>&1; then
        d_failed=$i
        break 2
      fi
    done
    sleep "$WAIT_READY"
    total_rss=0
    for j in $(seq 1 "$n"); do
      pid=$("$DOCKER_BIN" inspect -f '{{.State.Pid}}' "docker-density-$j" 2>/dev/null || echo 0)
      if [ "$pid" -gt 0 ]; then
        rss=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' || echo 0)
        total_rss=$((total_rss + rss * 1024))
      fi
    done
    DOCKER_MAX=$n
    DOCKER_PER_CONTAINER=$((total_rss / n))
  done
  if [ "$d_failed" -gt 0 ]; then
    warn "  Docker stopped at $((DOCKER_MAX + d_failed))"
  fi
  ok "  Docker density: $DOCKER_MAX containers, $((DOCKER_PER_CONTAINER / 1024)) KiB/container"
fi

# --- cleanup -----------------------------------------------------------------
note "== Cleaning up density containers =="
for i in $(seq 1 "$MAX_CONTAINERS"); do
  "$EXO_BIN" rm -f "exo-density-$i" >/dev/null 2>&1 || true
done
if have_docker; then
  for i in $(seq 1 "$DOCKER_MAX"); do
    "$DOCKER_BIN" rm -f "docker-density-$i" >/dev/null 2>&1 || true
  done
fi
ok "  Cleanup complete"

stop_exo_daemon

# --- emit JSON ---------------------------------------------------------------
DockerJson=""
if have_docker; then
  DockerJson=", \"docker\": {\"max_containers\": $DOCKER_MAX, \"per_container_kb\": $((DOCKER_PER_CONTAINER / 1024))}"
fi

cat > "$JSON" <<EOF
{
  "stamp": "$STAMP",
  "image": "$IMAGE",
  "command": "$COMMAND",
  "batch": $BATCH,
  "density": {
    "max_containers": $MAX_CONTAINERS,
    "failure_point": $FAILURE_POINT,
    "failure_reason": "$FAILURE_REASON",
    "host_total_rss_kb": $((TOTAL_RSS_AT_MAX / 1024)),
    "rss_per_container_kb": $((PER_CONTAINER_AT_MAX / 1024)),
    "host_used_rss_mb": $((HOST_RSS_AT_MAX / 1024)),
    "samples": [ $SAMPLES_JSON ]
  }$DockerJson
}
EOF
ok "Wrote $JSON"
