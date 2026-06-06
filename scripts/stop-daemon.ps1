# Script to stop the exo daemon in WSL2

$ErrorActionPreference = "Stop"

Write-Host "=== Stopping Exo Daemon ===" -ForegroundColor Cyan

# Stop the daemon using exo command
wsl /mnt/f/Software/exo/target/release/exo daemon --stop 2>&1

# Also try killing by process name
wsl -e bash -c "pkill -f 'exo daemon' || true" 2>$null | Out-Null

# Clean up socket and PID files
wsl -e bash -c "rm -f /tmp/exo-daemon.sock /tmp/exo-daemon.pid 2>/dev/null || true"

Write-Host "Daemon stopped" -ForegroundColor Green
