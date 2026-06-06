# Script to start the exo daemon in WSL2

$ErrorActionPreference = "Stop"

Write-Host "=== Exo Daemon Manager ===" -ForegroundColor Cyan

# Check if WSL2 is available
$wslVersion = wsl --version 2>$null
if (-not $?) {
    Write-Error "WSL2 is not installed"
    exit 1
}
Write-Success "WSL2 detected"

# Check if exo binary exists
$exoBinary = "/mnt/f/Software/exo/target/release/exo"
$result = wsl -e bash -c "test -f $exoBinary && echo 'found' || echo 'not found'"
if ($result -eq "not found") {
    Write-Error "Exo binary not found at $exoBinary"
    Write-Host "Build first with: docker run --rm -v 'F:\Software\exo:/workspace' -w '/workspace' rust:latest sh -c 'apt-get update -qq && apt-get install -y build-essential libseccomp-dev && cargo build --release'" -ForegroundColor Yellow
    exit 1
}
Write-Success "Exo binary found"

# Check current daemon status
Write-Host "`nChecking daemon status..." -ForegroundColor Cyan
$socketPath = "/tmp/exo-daemon.sock"
$daemonRunning = wsl -e bash -c "test -S $socketPath && echo 'running' || echo 'stopped'"

if ($daemonRunning -eq "running") {
    Write-Success "Daemon is already running"

    # Ping the daemon
    Write-Host "Pinging daemon..." -ForegroundColor Cyan
    $pingResult = wsl -e bash -c "echo '{\"type\":\"ping\"}' | socat - UNIX-CONNECT:$socketPath 2>/dev/null || echo 'failed'"
    if ($pingResult -match "pong") {
        Write-Success "Daemon is responding"
    } else {
        Write-Warning "Daemon is not responding, may need restart"
    }
} else {
    Write-Host "Daemon is not running" -ForegroundColor Yellow
    Write-Host "Starting daemon..." -ForegroundColor Cyan

    # Kill any stale daemon processes
    wsl -e bash -c "pkill -f exo daemon || true" 2>$null | Out-Null

    # Start the daemon in background
    wsl -e bash -c "nohup /mnt/f/Software/exo/target/release/exo daemon > /tmp/exo-daemon.log 2>&1 &" 2>&1 | Out-Null

    # Wait for daemon to start
    Write-Host "Waiting for daemon to start..." -ForegroundColor Cyan
    $started = $false
    for ($i = 0; $i -lt 10; $i++) {
        Start-Sleep -Milliseconds 500
        $result = wsl -e bash -c "test -S $socketPath && echo 'running' || echo 'stopped'"
        if ($result -eq "running") {
            $started = $true
            break
        }
    }

    if ($started) {
        Write-Success "Daemon started successfully"

        # Verify it responds
        $pingResult = wsl -e bash -c "echo '{\"type\":\"ping\"}' | socat - UNIX-CONNECT:$socketPath 2>/dev/null | grep pong"
        if ($pingResult) {
            Write-Success "Daemon is responding to pings"
        }
    } else {
        Write-Error "Failed to start daemon"
        Write-Host "Check logs: wsl cat /tmp/exo-daemon.log" -ForegroundColor Yellow
        exit 1
    }
}

Write-Host "`n=== Daemon Status ===" -ForegroundColor Green
Write-Host "Socket path: $socketPath" -ForegroundColor White
Write-Host "Log file: /tmp/exo-daemon.log" -ForegroundColor White
Write-Host "PID file: /tmp/exo-daemon.pid" -ForegroundColor White

Write-Host "`n=== Usage ===" -ForegroundColor Cyan
Write-Host "The daemon is now running and will persist across commands" -ForegroundColor White
Write-Host "This means:" -ForegroundColor White
Write-Host "  - No WSL2 startup overhead per command (~100-200ms saved)" -ForegroundColor Green
Write-Host "  - Connection reuse for better performance" -ForegroundColor Green
Write-Host "  - Fast command execution via Unix socket" -ForegroundColor Green

Write-Host "`nTo stop the daemon:" -ForegroundColor Yellow
Write-Host "  .\scripts\stop-daemon.ps1" -ForegroundColor White
Write-Host "  Or: wsl /mnt/f/Software/exo/target/release/exo daemon --stop" -ForegroundColor White
