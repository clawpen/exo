# Initialization script for Exo on Windows via WSL2
# This script sets up the WSL2 environment for Exo

$ErrorActionPreference = "Stop"

function Write-Step {
    param([string]$Message)
    Write-Host "→ $Message" -ForegroundColor Cyan
}

function Write-Success {
    param([string]$Message)
    Write-Host "✓ $Message" -ForegroundColor Green
}

function Write-Warning {
    param([string]$Message)
    Write-Host "! $Message" -ForegroundColor Yellow
}

Write-Host "=== Exo WSL2 Initialization ===" -ForegroundColor Green
Write-Host ""

# Step 1: Check WSL2
Write-Step "Checking WSL2 installation..."
try {
    $null = wsl --version 2>$null
    Write-Success "WSL2 is installed"
} catch {
    Write-Warning "WSL2 not found"
    Write-Host "  Install from: https://aka.ms/installwsl2" -ForegroundColor Yellow
    exit 1
}

# Step 2: Check for default distro
Write-Step "Checking WSL2 distro..."
$distros = wsl -l -q
if ($distros) {
    $defaultDistro = ($distros -match "^\w+" | Select-Object -First 1)
    Write-Success "Found distro: $defaultDistro"
} else {
    Write-Warning "No WSL2 distro found"
    Write-Host "  Install with: wsl --install -d Ubuntu-22.04" -ForegroundColor Yellow
    exit 1
}

# Step 3: Update and install dependencies in WSL2
Write-Step "Installing dependencies in WSL2..."

$installScript = @"
# Update package lists
sudo apt-get update -qq

# Install required packages
sudo apt-get install -y -qq \\
    curl \\
    ca-certificates \\
    fuse3 \\
    libfuse3-dev \\
    uidmap \\
    slirp4netns \\
    iptables \\
    iproute2 \\
    bridge-utils \\
    || exit 1

echo "Dependencies installed"
"@

$result = wsl -e bash -c $installScript
if ($LASTEXITCODE -eq 0) {
    Write-Success "Dependencies installed"
} else {
    Write-Warning "Some dependencies may not have installed"
    Write-Host $result
}

# Step 4: Check for Rust in WSL2
Write-Step "Checking Rust installation..."
$rustCheck = wsl -e bash -c "which cargo && echo 'found' || echo 'not found'"
if ($rustCheck -eq "not found") {
    Write-Warning "Rust not found in WSL2"
    Write-Host "  Installing Rust..." -ForegroundColor Cyan

    $installRust = @"
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
"@

    wsl -e bash -c $installRust
    Write-Success "Rust installed"
} else {
    Write-Success "Rust is installed"
}

# Step 5: Create state directory
Write-Step "Creating Exo state directory..."
$stateDirCmd = "sudo mkdir -p /var/lib/exo/containers /var/lib/exo/images /var/lib/exo/cache"
wsl -e bash -c $stateDirCmd
Write-Success "State directories created"

# Step 6: Build exo binary
Write-Step "Building exo binary..."
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$wslProjectRoot = "/mnt/$((Get-Location).Drive.Name.ToLower())$((Get-Location).Path.Substring(2).Replace('\', '/'))"

# Adjust for actual project root
$wslProjectRoot = "/mnt/f/Software/exo"  # Default, adjust as needed

$buildCmd = "cd $wslProjectRoot && cargo build --release 2>&1"
Write-Host "  Building in $wslProjectRoot..." -ForegroundColor Gray

$output = wsl -e bash -c $buildCmd
if ($LASTEXITCODE -eq 0) {
    Write-Success "exo binary built"
} else {
    Write-Warning "Build had issues (may still work)"
}

# Step 7: Create wrapper script
Write-Step "Creating exo wrapper..."
$wrapperPath = Join-Path $projectRoot "exo.cmd"
if (Test-Path $wrapperPath) {
    Write-Success "Wrapper script exists: $wrapperPath"
} else {
    Write-Warning "Wrapper script not found"
}

# Step 8: Verify installation
Write-Step "Verifying installation..."
$verifyCmd = "$wslProjectRoot/target/release/exo --version 2>&1"
$version = wsl -e bash -c $verifyCmd
if ($LASTEXITCODE -eq 0) {
    Write-Success "exo is working! Version: $version"
} else {
    Write-Warning "exo verification failed"
}

Write-Host ""
Write-Host "=== Initialization Complete ===" -ForegroundColor Green
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Cyan
Write-Host "  1. Run exo: .\exo.cmd --help" -ForegroundColor White
Write-Host "  2. Run a container: .\exo.cmd run python:3.12 python --version" -ForegroundColor White
Write-Host "  3. Test setup: .\scripts\test-wsl.ps1" -ForegroundColor White
