#!/bin/bash
# Performance benchmark script for exo daemon vs direct mode
# Run this inside WSL2

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

echo -e "${CYAN}"
echo "╔══════════════════════════════════════════════════════════╗"
echo "║     Exo Performance Benchmark: Daemon vs Direct Mode         ║"
echo "╚══════════════════════════════════════════════════════════╝"
echo -e "${NC}"

EXO_BIN="/mnt/f/Software/exo/target/release/exo"
SOCKET_PATH="/tmp/exo-daemon.sock"

# Function to measure command execution time
measure_time() {
    local name="$1"
    local command="$2"

    echo -e "${CYAN}Testing: $name${NC}"

    # Warmup
    eval "$command" > /dev/null 2>&1 || true

    # Measure 5 runs
    local total=0
    for i in 1 5; do
        local start=$(date +%s%3N)
        eval "$command" > /dev/null 2>&1 || true
        local end=$(date +%s%3N)
        local elapsed=$((end - start))
        total=$((total + elapsed))
        echo "  Run $i: ${elapsed}ms"
    done

    local avg=$((total / 5))
    echo -e "  ${GREEN}Average: ${avg}ms${NC}"
    echo ""
}

# Check if daemon is running
if [ -S "$SOCKET_PATH" ]; then
    echo -e "${GREEN}✓ Daemon is running${NC}"
    DAEMON_AVAILABLE=true
else
    echo -e "${YELLOW}⚠ Daemon is not running${NC}"
    echo "  Start daemon with: ./scripts/start-daemon.ps1"
    DAEMON_AVAILABLE=false
fi

echo ""
echo -e "${YELLOW}=== Benchmarking Direct Mode (spawning wsl per command) ===${NC}"

# Direct mode: spawn wsl for each command
measure_time "Direct mode: ping" "wsl --user root -- /mnt/f/Software/exo/target/release/exo --version"

measure_time "Direct mode: list" "wsl --user root -- /mnt/f/Software/exo/target/release/exo list"

if [ "$DAEMON_AVAILABLE" = true ]; then
    echo ""
    echo -e "${YELLOW}=== Benchmarking Daemon Mode (Unix socket, no wsl spawn) ===${NC}"

    # Daemon mode: use socat to talk to Unix socket
    measure_time "Daemon mode: ping" "echo '{\"type\":\"ping\"}' | socat - UNIX-CONNECT:$SOCKET_PATH 2>/dev/null"

    measure_time "Daemon mode: list" "echo '{\"type\":\"list\",\"content\":{\"all\":false}}' | socat - UNIX-CONNECT:$SOCKET_PATH 2>/dev/null"
else
    echo ""
    echo "Daemon not available. Skipping daemon benchmarks."
fi

echo -e "${CYAN}=== Summary ===${NC}"
echo "The daemon mode should show significantly faster times"
echo "because it avoids the WSL2 startup overhead (~100-200ms per call)."
echo ""
echo "Expected improvement:"
echo "  Direct mode: 200-400ms per command (WSL2 startup)"
echo "  Daemon mode: 5-20ms per command (Unix socket)"
echo "  Speedup: 10-40x faster"
