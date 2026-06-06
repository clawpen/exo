# Exo Windows Build Script (PowerShell)
# This script orchestrates building exo via WSL2 for Windows users

$ErrorActionPreference = "Stop"

Write-Host "=== Exo Windows Build Script ===" -ForegroundColor Green

# Function to check if WSL2 is installed
function Test-WSL2 {
    try {
        $null = wsl --version 2>$null
        return $true
    } catch {
        return $false
    }
}

# Function to convert Windows path to WSL path
function ConvertTo-WSLPath {
    param([string]$Path)

    # Convert backslashes to forward slashes
    $wslPath = $Path -replace '\\', '/'

    # Convert drive letter (e.g., C: -> /mnt/c)
    if ($wslPath -match '^([A-Z]):(.*)$') {
        $drive = $matches[1].ToLower()
        $rest = $matches[2]
        $wslPath = "/mnt/$drive$rest"
    }

    return $wslPath
}

# Check WSL2 is installed
if (-not (Test-WSL2)) {
    Write-Host "Error: WSL2 is not installed" -ForegroundColor Red
    Write-Host "Install WSL2 from: https://aka.ms/installwsl2" -ForegroundColor Yellow
    exit 1
}

Write-Host "✓ WSL2 detected" -ForegroundColor Green

# Get the project root
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$WSLProjectRoot = ConvertTo-WSLPath -Path $ProjectRoot

Write-Host "Project root: $ProjectRoot" -ForegroundColor Cyan
Write-Host "WSL path: $WSLProjectRoot" -ForegroundColor Cyan

# Check if build script exists in WSL
Write-Host "`nBuilding exo in WSL2..." -ForegroundColor Green

$buildCommand = "cd $WSLProjectRoot && bash scripts/build-wsl.sh"

# Execute build in WSL2
$output = wsl -e bash -c $buildCommand

if ($LASTEXITCODE -eq 0) {
    Write-Host "`n✓ Build successful!" -ForegroundColor Green
    Write-Host $output

    # Create/update the Windows wrapper script
    $wrapperScript = Join-Path $ProjectRoot "exo.cmd"
    if (-not (Test-Path $wrapperScript)) {
        Write-Host "`nWarning: exo.cmd wrapper not found" -ForegroundColor Yellow
    } else {
        Write-Host "✓ Windows wrapper: $wrapperScript" -ForegroundColor Green
    }

    Write-Host "`nYou can now run exo from Windows using:" -ForegroundColor Cyan
    Write-Host "  .\exo.cmd --help" -ForegroundColor White
    Write-Host "`nOr add the project directory to your PATH to use 'exo' anywhere."
} else {
    Write-Host "`n✗ Build failed!" -ForegroundColor Red
    Write-Host $output
    exit 1
}
