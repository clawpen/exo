#!/usr/bin/env bash
#
# M3 admission-control load test.
#
# Fires N concurrent `run` requests at the daemon and counts:
#   OK             — admission gate accepted, container start attempted
#   BUSY           — start-concurrency cap rejected (M3 working as designed)
#   ERROR          — passed admission, failed for other reasons (e.g. no root)
#   CONN_BUSY      — connection cap rejected before request was even read
#
# Usage: scripts/loadtest-admission.sh [num_requests] [concurrent_starts_cap]
#   default: 50 requests, daemon's default cap of 32

set -u

NUM=${1:-50}
START_CAP=${2:-32}
EXO_BIN="${EXO_BIN:-$(pwd)/target/release/exo}"
SOCKET="${SOCKET:-/tmp/exo-daemon.sock}"
LOG=/tmp/exo-loadtest-daemon.log
RESP_DIR=/tmp/exo-loadtest-resp

if [[ ! -x "$EXO_BIN" ]]; then
    echo "exo binary not found at $EXO_BIN" >&2
    echo "Build first: cargo build --release -p exo" >&2
    exit 1
fi
if ! command -v socat >/dev/null; then
    echo "socat is required" >&2
    exit 1
fi

# Clean up stale state from previous runs.
rm -f "$SOCKET"
rm -rf "$RESP_DIR" && mkdir -p "$RESP_DIR"

echo "Starting daemon (cap=$START_CAP)..."
EXO_MAX_CONCURRENT_STARTS=$START_CAP "$EXO_BIN" daemon --foreground >"$LOG" 2>&1 &
DAEMON_PID=$!

# Wait for socket.
for i in $(seq 1 50); do
    if [[ -S "$SOCKET" ]]; then break; fi
    sleep 0.1
done
if [[ ! -S "$SOCKET" ]]; then
    echo "Daemon failed to start. Last log lines:"
    tail -20 "$LOG"
    kill $DAEMON_PID 2>/dev/null
    exit 1
fi

echo "Daemon up. Firing $NUM concurrent run requests..."
START=$(date +%s.%N)

for i in $(seq 1 $NUM); do
    (
        REQ='{"type":"run","content":{"spec":{"name":"loadtest-'$i'","image":"alpine","workdir":"/","env":[],"command":["sleep","30"],"mounts":[]}}}'
        printf '%s\n' "$REQ" \
            | socat -t 5 - UNIX-CONNECT:"$SOCKET" \
            > "$RESP_DIR/resp-$i.json" 2>"$RESP_DIR/err-$i"
    ) &
done
wait

END=$(date +%s.%N)
ELAPSED=$(awk "BEGIN{printf \"%.2f\", $END - $START}")

# Categorize. Use specific markers from execute_run / run_server messages.
OK=$(grep -l '"type":"ok"' "$RESP_DIR"/resp-*.json 2>/dev/null | wc -l)
BUSY_START=$(grep -l 'start-concurrency cap' "$RESP_DIR"/resp-*.json 2>/dev/null | wc -l)
BUSY_CONN=$(grep -l 'connection capacity' "$RESP_DIR"/resp-*.json 2>/dev/null | wc -l)
TOTAL_ERR=$(grep -l '"type":"error"' "$RESP_DIR"/resp-*.json 2>/dev/null | wc -l)
OTHER_ERR=$((TOTAL_ERR - BUSY_START - BUSY_CONN))
EMPTY=$(find "$RESP_DIR" -name 'resp-*.json' -size 0 | wc -l)

echo
echo "=== Results ==="
echo "Elapsed:     ${ELAPSED}s"
echo "Requests:    $NUM"
echo "OK:          $OK   (admission accepted, container started or attempted)"
echo "BUSY_START:  $BUSY_START   (M3 start-cap rejection — expected ~$((NUM - START_CAP)) when NUM > cap)"
echo "BUSY_CONN:   $BUSY_CONN   (M3 connection-cap rejection)"
echo "OTHER_ERR:   $OTHER_ERR   (passed admission, failed downstream — usually 'no root' for cgroup writes)"
echo "EMPTY:       $EMPTY   (socat timed out or nothing returned)"

echo
echo "=== Event log (last 15 entries via 'exo events') ==="
"$EXO_BIN" events --limit 15 2>&1 | head -20

echo
echo "Stopping daemon (PID $DAEMON_PID)..."
kill $DAEMON_PID 2>/dev/null
wait $DAEMON_PID 2>/dev/null
echo "Done. Daemon log: $LOG ; Response dir: $RESP_DIR"
