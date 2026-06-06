# Test script for Exo on Windows via WSL2
# This script verifies that the WSL2 backend is working correctly

$ErrorActionPreference = "Stop"

function Write-Status {
    param(
        [string]$Message,
        [string]$Status = "Info"
    )

    $color = switch ($Status) {
        "Success" { "Green" }
        "Error" { "Red" }
        "Warning" { "Yellow" }
        default { "Cyan" }
    }

    $symbol = switch ($Status) {
        "Success" { "✓" }
        "Error" { "✗" }
        "Warning" { "!" }
        default { "→" }
    }

    Write-Host "$symbol $Message" -ForegroundColor $color
}

Write-Host "=== Exo Windows/WSL2 Test ===" -ForegroundColor Green
Write-Host ""

# Test 1: Check WSL2 installation
Write-Status "Checking WSL2 installation..."
try {
    $wslVersion = wsl --version 2>$null
    if ($LASTEXITCODE -eq 0) {
        Write-Status "WSL2 is installed" "Success"
    } else {
        Write-Status "WSL2 not found" "Error"
        exit 1
    }
} catch {
    Write-Status "Failed to check WSL2" "Error"
    exit 1
}

# Test 2: Check default distro
Write-Status "Checking WSL2 distro..."
$distros = wsl -l -q
if ($distros) {
    Write-Status "Found distro(s): $($distros -join ', ')" "Success"
} else {
    Write-Status "No WSL2 distro found" "Warning"
    Write-Host "  Install with: wsl --install -d Ubuntu" -ForegroundColor Yellow
}

# Test 3: Check if exo binary exists
Write-Status "Checking exo binary..."
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$exoBinary = Join-Path $projectRoot "target\release\exo"

if (Test-Path $exoBinary) {
    Write-Status "exo binary found" "Success"
} else {
    Write-Status "exo binary not found - run build first" "Warning"
}

# Test 4: Check if exo is accessible in WSL
Write-Status "Checking exo in WSL..."
$result = wsl -e bash -c "which exo || echo 'not found'"
if ($result -ne "not found") {
    Write-Status "exo is in WSL PATH: $result" "Success"
} else {
    Write-Status "exo not in WSL PATH" "Warning"
}

# Test 5: Check exo version
Write-Status "Testing exo command..."
$version = wsl -e bash -c "cd /mnt/f/Software/exo 2>/dev/null && ./target/release/exo --version 2>/dev/null || echo 'error'"
if ($version -ne "error") {
    Write-Status "exo version: $version" "Success"
} else {
    Write-Status "exo command failed" "Error"
}

# Test 6: Check container runtime
Write-Status "Checking container runtime support..."
$hasNamespaces = wsl -e bash -c "ls /proc/self/ns/ 2>/dev/null | wc -l"
if ($hasNamespaces -gt 0) {
    Write-Status "Namespaces available: $hasNamespaces types" "Success"
} else {
    Write-Status "Namespaces not available" "Error"
}

# Test 7: Check cgroups v2
Write-Status "Checking cgroups..."
$cgroupv2 = wsl -e bash -c "test -f /sys/fs/cgroup/cgroup.controllers && echo 'v2' || echo 'v1'"
Write-Status "Cgroups version: $cgroupv2" "Success"

# Test 8: Check for fuse (for rootless containers)
Write-Status "Checking FUSE support..."
$fuseResult = wsl -e bash -c "which fuse-overlayfs 2>/dev/null && echo 'found' || echo 'not found'"
if ($fuseResult -eq "found") {
    Write-Status "fuse-overlayfs installed" "Success"
} else {
    Write-Status "fuse-overlayfs not installed (optional, for rootless)" "Warning"
}

# Test 9: Check for binfmt_misc (for multi-arch)
Write-Status "Checking binfmt_misc (for multi-arch support)..."
$binfmt = wsl -e bash -c "ls /proc/sys/fs/binfmt_misc/ 2>/dev/null | wc -l"
if ($binfmt -gt 0) {
    Write-Status "binfmt_misc handlers: $binfmt" "Success"
} else {
    Write-Status "binfmt_misc not configured" "Warning"
}

Write-Host ""
Write-Host "=== Test Complete ===" -ForegroundColor Green
Write-Host ""
Write-Host "To build exo: .\scripts\build-wsl.ps1" -ForegroundColor Cyan
Write-Host "To run exo: .\exo.cmd --help" -ForegroundColor Cyan
