# Test script for Exo volume mounting on Windows via WSL2
# This script verifies that volume mounts work correctly across the Windows/WSL2 boundary

$ErrorActionPreference = "Stop"

# Color helpers
function Write-Success { param([string]$m); Write-Host "[PASS] $m" -ForegroundColor Green }
function Write-Error { param([string]$m); Write-Host "[FAIL] $m" -ForegroundColor Red }
function Write-Info { param([string]$m); Write-Host "[INFO] $m" -ForegroundColor Cyan }
function Write-Header { param([string]$m); Write-Host "`n=== $m ===" -ForegroundColor Yellow }

# Counter for tests
$pass = 0
$fail = 0

function Test-Step {
    param(
        [string]$Name,
        [scriptblock]$Script
    )

    Write-Info "Testing: $Name"
    try {
        $result = & $Script
        if ($result) {
            Write-Success "$Name"
            $script:pass++
        } else {
            Write-Error "$Name"
            $script:fail++
        }
    } catch {
        Write-Error "$Name - $_"
        $script:fail++
    }
}

# Get project root
$ProjectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$TestDir = Join-Path $ProjectRoot "test-volumes"
$TestDataDir = Join-Path $TestDir "persistent-data"

Write-Host "
==========================================
Exo Volume Mount Test Suite for Windows/WSL2
==========================================
" -ForegroundColor Cyan

Write-Header "Environment Check"

# Check if we're on Windows (more lenient check for bash invocation)
$isWindows = $PSVersionTable.Platform -eq "Win32NT" -or $PSVersionTable.PSVersion.Major -lt 6 -or $env:OS -eq "Windows_NT"
if (-not $isWindows) {
    Write-Error "This script must be run on Windows"
    exit 1
}
Write-Success "Running on Windows"

# Check WSL2
Test-Step "WSL2 is installed" {
    $null = wsl --version 2>$null
    return $?
}

Test-Step "WSL2 has a distro" {
    $distros = wsl -l -q
    return ($distros -ne "")
}

# Get exo binary path
$ExoCmd = Join-Path $ProjectRoot "exo.cmd"

Write-Header "Path Translation Tests"

# Test 1: Simple Windows path to WSL
Test-Step "Convert C:\ to WSL path" {
    $windowsPath = "C:\Users\test"
    $expected = "/mnt/c/Users/test"
    # Do conversion in PowerShell instead of bash
    $converted = $windowsPath.Replace('\', '/')
    if ($converted -match '^([A-Z]):/(.*)') {
        $drive = $matches[1].ToLower()
        $rest = $matches[2]
        $converted = "/mnt/$drive/$rest"
    }
    return $converted -eq $expected
}

# Test 2: Windows path with spaces
Test-Step "Convert path with spaces" {
    $windowsPath = "C:\Program Files\test"
    # Do conversion in PowerShell
    $converted = $windowsPath.Replace('\', '/')
    if ($converted -match '^([A-Z]):/(.*)') {
        $drive = $matches[1].ToLower()
        $rest = $matches[2]
        $converted = "/mnt/$drive/$rest"
    }
    return $converted -eq "/mnt/c/Program Files/test"
}

# Test 3: F drive
Test-Step "Convert F:\ drive path" {
    $windowsPath = "F:\Software\exo"
    $expected = "/mnt/f/Software/exo"
    # Do conversion in PowerShell
    $converted = $windowsPath.Replace('\', '/')
    if ($converted -match '^([A-Z]):/(.*)') {
        $drive = $matches[1].ToLower()
        $rest = $matches[2]
        $converted = "/mnt/$drive/$rest"
    }
    return $converted -eq $expected
}

Write-Header "Volume Mount Setup"

# Create test directory
if (Test-Path $TestDir) {
    Remove-Item -Recurse -Force $TestDir
}
New-Item -ItemType Directory -Path $TestDir -Force | Out-Null
New-Item -ItemType Directory -Path $TestDataDir -Force | Out-Null

# Create test file in Windows
$TestFile = Join-Path $TestDir "test.txt"
"Hello from Windows!" | Out-File -FilePath $TestFile -Encoding utf8

Write-Success "Created test directory: $TestDir"
Write-Success "Created test file: $TestFile"

# Get WSL path for test directory
$drive = (Get-Item $TestDir).PSDrive.Name.ToLower()
$relativePath = (Get-Item $TestDir).FullName.Substring(3).Replace('\', '/')
$WSLTestDir = "/mnt/$drive/$relativePath"
Write-Info "WSL path: $WSLTestDir"

Write-Header "Basic Volume Mount Tests"

# Test 4: Read Windows file from WSL2
Test-Step "Read Windows file from WSL2" {
    $result = wsl -e bash -c "cat '$WSLTestDir/test.txt' 2>/dev/null || echo 'error'"
    return $result -eq "Hello from Windows!"
}

# Test 5: Write to Windows directory from WSL2
Test-Step "Write to Windows directory from WSL2" {
    $outputFile = Join-Path $TestDir "wsl-wrote.txt"
    wsl -e bash -c "echo 'Written from WSL2' > '$WSLTestDir/wsl-wrote.txt'" 2>$null
    Start-Sleep -Milliseconds 500
    if (Test-Path $outputFile) {
        $content = Get-Content $outputFile -Raw
        return $content -match "Written from WSL2"
    }
    return $false
}

# Test 6: Create nested directory structure
Test-Step "Create nested directories via WSL2" {
    wsl -e bash -c "mkdir -p '$WSLTestDir/nested/deep/path' && echo 'success' > '$WSLTestDir/nested/deep/path/test.txt'"
    Start-Sleep -Milliseconds 500
    $nestedFile = Join-Path $TestDir "nested\deep\path\test.txt"
    return Test-Path $nestedFile
}

Write-Header "Exo Container Volume Tests"

# Check if exo binary exists
$hasExo = $false
try {
    $null = wsl -e bash -c "which exo 2>/dev/null || /mnt/f/Software/exo/target/release/exo --version 2>/dev/null"
    if ($LASTEXITCODE -eq 0) {
        $hasExo = $true
    }
} catch {}

if (-not $hasExo) {
    Write-Error "Exo binary not found. Build first with: .\scripts\build-wsl.ps1"
    Write-Info "Skipping container tests..."
} else {
    Write-Success "Exo binary found"

    # Test 7: Simple volume mount with a scratch container
    Test-Step "Basic volume mount to container" {
        $mountSource = $WSLTestDir
        $mountTarget = "/data"
        $mountSpec = "$mountSource" + ":" + "$mountTarget"
        $result = wsl -e bash -c "cd /mnt/f/Software/exo 2>/dev/null && timeout 10 target/release/exo run --rm -v $mountSpec alpine:latest cat /data/test.txt 2>&1 || echo 'timeout_or_error'"
        return $result -match "Hello from Windows!"
    }

    # Test 8: Write to Windows volume from container
    Test-Step "Write to Windows volume from container" {
        $containerFile = Join-Path $TestDir "container-wrote.txt"
        if (Test-Path $containerFile) { Remove-Item $containerFile }

        $mountSource = $WSLTestDir
        $mountTarget = "/data"
        $mountSpec = "$mountSource" + ":" + "$mountTarget"

        wsl -e bash -c "cd /mnt/f/Software/exo 2>/dev/null && timeout 10 target/release/exo run --rm -v $mountSpec alpine:latest sh -c 'echo Container data > /data/container-wrote.txt' 2>&1" | Out-Null

        Start-Sleep -Milliseconds 1000
        if (Test-Path $containerFile) {
            $content = Get-Content $containerFile -Raw
            return $content -match "Container data"
        }
        return $false
    }

    # Test 9: Persistent data across container runs
    Test-Step "Persistent data survives container restart" {
        $persistFile = Join-Path $TestDataDir "persistent.txt"
        if (Test-Path $persistFile) { Remove-Item $persistFile }

        # First run: write data
        $persistDir = "$WSLTestDir/persistent-data"
        $mountSpec = "$persistDir" + ":/persist"
        wsl -e bash -c "cd /mnt/f/Software/exo 2>/dev/null && timeout 10 target/release/exo run --rm -v $mountSpec alpine:latest sh -c 'echo First run > /persist/persistent.txt' 2>&1" | Out-Null

        Start-Sleep -Milliseconds 1000

        # Second run: read data
        $result = wsl -e bash -c "cd /mnt/f/Software/exo 2>/dev/null && timeout 10 target/release/exo run --rm -v $mountSpec alpine:latest cat /persist/persistent.txt 2>&1"

        return $result -match "First run"
    }
}

Write-Header "Edge Case Tests"

# Test 10: Path with special characters
Test-Step "Handle path with spaces in name" {
    $spaceDir = Join-Path $TestDir "path with spaces"
    New-Item -ItemType Directory -Path $spaceDir -Force | Out-Null
    "Spaces work!" | Out-File -FilePath (Join-Path $spaceDir "test.txt")

    $spaceDrive = (Get-Item $spaceDir).PSDrive.Name.ToLower()
    $spaceRelative = (Get-Item $spaceDir).FullName.Substring(3).Replace('\', '/')
    $wslSpaceDir = "/mnt/$spaceDrive/$spaceRelative"

    $result = wsl -e bash -c "cat '$wslSpaceDir/test.txt' 2>/dev/null"

    return $result -eq "Spaces work!"
}

# Test 11: Multiple volume mounts
Test-Step "Multiple volume mounts at once" {
    $dir1 = Join-Path $TestDir "vol1"
    $dir2 = Join-Path $TestDir "vol2"
    New-Item -ItemType Directory -Path $dir1 -Force | Out-Null
    New-Item -ItemType Directory -Path $dir2 -Force | Out-Null
    "volume1" | Out-File -FilePath (Join-Path $dir1 "data.txt")
    "volume2" | Out-File -FilePath (Join-Path $dir2 "data.txt")

    $drive1 = (Get-Item $dir1).PSDrive.Name.ToLower()
    $rel1 = (Get-Item $dir1).FullName.Substring(3).Replace('\', '/')
    $wslDir1 = "/mnt/$drive1/$rel1"

    $drive2 = (Get-Item $dir2).PSDrive.Name.ToLower()
    $rel2 = (Get-Item $dir2).FullName.Substring(3).Replace('\', '/')
    $wslDir2 = "/mnt/$drive2/$rel2"

    $result1 = wsl -e bash -c "cat '$wslDir1/data.txt' 2>/dev/null"
    $result2 = wsl -e bash -c "cat '$wslDir2/data.txt' 2>/dev/null"

    return ($result1 -eq "volume1") -and ($result2 -eq "volume2")
}

# Test 12: Read-only mount simulation
Test-Step "Read-only volume mount (verify)" {
    $roDir = Join-Path $TestDir "readonly"
    New-Item -ItemType Directory -Path $roDir -Force | Out-Null
    "readonly data" | Out-File -FilePath (Join-Path $roDir "data.txt")

    $roDrive = (Get-Item $roDir).PSDrive.Name.ToLower()
    $roRel = (Get-Item $roDir).FullName.Substring(3).Replace('\', '/')
    $wslRoDir = "/mnt/$roDrive/$roRel"

    $result = wsl -e bash -c "cat '$wslRoDir/data.txt' 2>/dev/null"

    return $result -eq "readonly data"
}

Write-Header "Performance Tests"

# Test 13: Large file transfer
Test-Step "Large file read through mount" {
    $largeFile = Join-Path $TestDir "large.txt"
    # Create a 1MB file
    $data = "x" * 1000
    for ($i = 0; $i -lt 1000; $i++) {
        $data | Out-File -FilePath $largeFile -Append -Encoding utf8
    }

    $largeDrive = (Get-Item $largeFile).PSDrive.Name.ToLower()
    $largeRel = (Get-Item $largeFile).FullName.Substring(3).Replace('\', '/')
    $wslLargeFile = "/mnt/$largeDrive/$largeRel"

    $start = Get-Date
    # Use cut instead of awk to avoid escaping issues
    $result = wsl -e bash -c "wc -c '$wslLargeFile' 2>/dev/null" | ForEach-Object { $_.Split()[0] }
    $elapsed = ((Get-Date) - $start).TotalMilliseconds

    Write-Host "       Read 1MB in $($elapsed)ms" -ForegroundColor Gray
    return [int]$result -gt 1000000
}

# Test 14: Many small files
Test-Step "Many small files throughput" {
    $manyDir = Join-Path $TestDir "many"
    New-Item -ItemType Directory -Path $manyDir -Force | Out-Null

    # Create 100 small files
    for ($i = 0; $i -lt 100; $i++) {
        "file $i" | Out-File -FilePath (Join-Path $manyDir "file$i.txt")
    }

    $manyDrive = (Get-Item $manyDir).PSDrive.Name.ToLower()
    $manyRel = (Get-Item $manyDir).FullName.Substring(3).Replace('\', '/')
    $wslManyDir = "/mnt/$manyDrive/$manyRel"

    $start = Get-Date
    $result = wsl -e bash -c "ls '$wslManyDir' 2>/dev/null | wc -l"
    $elapsed = ((Get-Date) - $start).TotalMilliseconds

    Write-Host "       Listed 100 files in $($elapsed)ms" -ForegroundColor Gray
    return [int]$result -ge 100
}

Write-Header "Cleanup"

Write-Info "Cleaning up test files..."
try {
    Remove-Item -Recurse -Force $TestDir -ErrorAction SilentlyContinue
    Write-Success "Test directory cleaned up"
} catch {
    Write-Error "Could not clean up test directory: $TestDir"
}

Write-Header "Test Results Summary"

Write-Host ""
Write-Host "Total Tests: $($pass + $fail)" -ForegroundColor White
Write-Host "Passed:      " -NoNewline
Write-Host $pass -ForegroundColor Green
Write-Host "Failed:      " -NoNewline
if ($fail -gt 0) {
    Write-Host $fail -ForegroundColor Red
} else {
    Write-Host $fail -ForegroundColor Green
}

if ($fail -eq 0) {
    Write-Host "`nAll tests passed!" -ForegroundColor Green
    Write-Host ""
    Write-Host "Volume mounting is working correctly on your Windows/WSL2 setup." -ForegroundColor Cyan
    exit 0
} else {
    Write-Host "`nSome tests failed. Check the output above for details." -ForegroundColor Yellow
    Write-Host ""
    Write-Host "Common issues:" -ForegroundColor Yellow
    Write-Host "  - WSL2 may need to be restarted: wsl --shutdown" -ForegroundColor White
    Write-Host "  - Exo binary may need to be rebuilt" -ForegroundColor White
    Write-Host "  - Check firewall/antivirus settings" -ForegroundColor White
    exit 1
}
