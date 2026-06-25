param(
    [int]$Runs = 5,
    [string]$Image = "alpine:3.20",
    [string]$ExoBin = "",
    [string]$OutDir = "",
    [switch]$IncludeVolume,
    [switch]$IncludeFirstRun,
    [string]$ColdImage = "",
    [switch]$IncludeAgentImages,
    [string[]]$AgentImages = @("exo-agent:docker-test", "exo-agent:docker-test-slim"),
    [switch]$IncludeDaemon,
    [switch]$AllowColdDelete,
    [switch]$EmitMarkdown,
    [switch]$DockerViaWsl
)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $repo "results" }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

if (-not $ExoBin) {
    $wrapper = Join-Path $repo "exo.cmd"
    if (Test-Path $wrapper) { $ExoBin = $wrapper } else { $ExoBin = "exo" }
}

function Invoke-NativeQuiet {
    param([scriptblock]$Script)

    # Exo logs normal INFO/WARN lines to stderr. Windows PowerShell can surface
    # native stderr as error records when $ErrorActionPreference is Stop, so run
    # native benchmark commands with non-terminating stderr and check exit codes
    # explicitly instead.
    $oldEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Script 1>$null 2>$null
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $oldEap
    }

    if ($null -ne $code -and $code -ne 0) {
        throw "native command exited with code $code"
    }
}

function Invoke-NativeCapture {
    param([scriptblock]$Script)

    $oldEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = & $Script 2>$null
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $oldEap
    }

    if ($null -ne $code -and $code -ne 0) {
        throw "native command exited with code $code"
    }
    return ($out -join "`n")
}

function Measure-OneMs {
    param([scriptblock]$Script)
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Invoke-NativeQuiet $Script
    $sw.Stop()
    return [int][math]::Round($sw.Elapsed.TotalMilliseconds)
}

function Measure-SamplesMs {
    param(
        [scriptblock]$Script,
        [int]$Count
    )

    $samples = @()
    for ($i = 0; $i -lt $Count; $i++) {
        $samples += Measure-OneMs $Script
    }

    return $samples
}

function Convert-SamplesToStats {
    param([int[]]$Samples)

    if (-not $Samples -or $Samples.Count -eq 0) { return $null }

    $sorted = @($Samples | Sort-Object)
    $medianIndex = [math]::Floor(($sorted.Count - 1) / 2)
    $p95Index = [math]::Ceiling($sorted.Count * 0.95) - 1
    if ($p95Index -lt 0) { $p95Index = 0 }
    if ($p95Index -ge $sorted.Count) { $p95Index = $sorted.Count - 1 }

    return [ordered]@{
        samples_ms = $Samples
        min_ms = [int]$sorted[0]
        median_ms = [int]$sorted[$medianIndex]
        p95_ms = [int]$sorted[$p95Index]
        max_ms = [int]$sorted[$sorted.Count - 1]
    }
}

function Test-CommandAvailable {
    param([string]$Command)
    if (Test-Path $Command) { return $true }
    return [bool](Get-Command $Command -ErrorAction SilentlyContinue)
}

function Test-DockerHealthy {
    # Returns $true only if the Docker daemon answers with a server version.
    # We check stdout content rather than $proc.ExitCode because Start-Process
    # with redirected streams can report a null ExitCode even on success.
    param([int]$TimeoutSec = 60)
    $outF = [System.IO.Path]::GetTempFileName()
    $errF = [System.IO.Path]::GetTempFileName()
    try {
        if ($DockerViaWsl) {
            $file = "wsl"
            $args = @("-e", "docker", "version", "--format", "{{.Server.Version}}")
        } else {
            $file = "docker"
            $args = @("version", "--format", "{{.Server.Version}}")
        }
        $proc = Start-Process -FilePath $file -ArgumentList $args `
            -NoNewWindow -PassThru -RedirectStandardOutput $outF -RedirectStandardError $errF
        if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
            try { $proc | Stop-Process -ErrorAction SilentlyContinue } catch {}
            return $false
        }
        $server = (Get-Content $outF -Raw -ErrorAction SilentlyContinue)
        return ($server -match '\d+\.\d+')
    } catch {
        return $false
    } finally {
        Remove-Item $outF, $errF -ErrorAction SilentlyContinue
    }
}

function Test-NativeResponsive {
    # Bounded health probe so an unresponsive Docker daemon aborts the run fast
    # instead of letting later rmi/pull/run calls hang for many minutes.
    param(
        [string]$File,
        [string[]]$Arguments,
        [int]$TimeoutSec = 30
    )
    $outF = [System.IO.Path]::GetTempFileName()
    $errF = [System.IO.Path]::GetTempFileName()
    try {
        $proc = Start-Process -FilePath $File -ArgumentList $Arguments -NoNewWindow -PassThru `
            -RedirectStandardOutput $outF -RedirectStandardError $errF
        if (-not $proc.WaitForExit($TimeoutSec * 1000)) {
            # Stop the specific probe process we started (by object, not by raw
            # PID list) so a hung client does not linger.
            try { $proc | Stop-Process -ErrorAction SilentlyContinue } catch {}
            return $false
        }
        return ($proc.ExitCode -eq 0)
    } catch {
        return $false
    } finally {
        Remove-Item $outF, $errF -ErrorAction SilentlyContinue
    }
}

function Convert-ToWslPath {
    param([string]$Path)
    $resolved = (Resolve-Path $Path).Path
    if ($resolved -match '^([A-Za-z]):\\(.*)$') {
        $drive = $Matches[1].ToLowerInvariant()
        $rest = $Matches[2].Replace([char]92, '/')
        return "/mnt/$drive/$rest"
    }
    return $resolved.Replace([char]92, '/')
}

function Invoke-DockerQuiet {
    param([string[]]$DockerArgs)
    if ($DockerViaWsl) {
        Invoke-NativeQuiet { wsl -e docker @DockerArgs }
    } else {
        Invoke-NativeQuiet { docker @DockerArgs }
    }
}

function Invoke-DockerCapture {
    param([string[]]$DockerArgs)
    if ($DockerViaWsl) {
        return Invoke-NativeCapture { wsl -e docker @DockerArgs }
    }
    return Invoke-NativeCapture { docker @DockerArgs }
}

function Invoke-DockerHost {
    param([string[]]$DockerArgs)
    if ($DockerViaWsl) {
        wsl -e docker @DockerArgs | Out-Host
    } else {
        docker @DockerArgs | Out-Host
    }
}

function Get-DockerImageBytesOrNull {
    param([string]$Name)
    try {
        $raw = Invoke-DockerCapture @("image", "inspect", $Name, "--format", "{{.Size}}")
        return [int64]$raw.Trim()
    } catch {
        return $null
    }
}

$exoAvailable = Test-CommandAvailable $ExoBin

Write-Host "==> Docker baseline"
# Probe the lightweight /version endpoint rather than /info: a half-ready Docker
# Desktop often answers version quickly while full `docker info` still stalls.
if (-not (Test-DockerHealthy -TimeoutSec 60)) {
    throw "Docker did not respond within 60s. Ensure Docker Desktop is running and healthy before benchmarking (this guard prevents multi-minute hangs on rmi/pull/run)."
}
Invoke-DockerHost @("pull", $Image)
$dockerSamples = Measure-SamplesMs -Count $Runs -Script { Invoke-DockerQuiet @("run", "--rm", $Image, "true") }
$dockerStats = Convert-SamplesToStats $dockerSamples
$dockerImageBytes = Get-DockerImageBytesOrNull $Image
Write-Host "Docker spawn median: $($dockerStats.median_ms) ms (min=$($dockerStats.min_ms), p95=$($dockerStats.p95_ms), max=$($dockerStats.max_ms))"
if ($null -ne $dockerImageBytes) {
    Write-Host "Docker image size:    $([math]::Round($dockerImageBytes / 1MB, 1)) MiB"
}

$exoStats = $null
if ($exoAvailable) {
    Write-Host "==> Exo comparison"
    try {
        Invoke-NativeQuiet { & $ExoBin pull $Image }
        $exoSamples = Measure-SamplesMs -Count $Runs -Script { & $ExoBin run --rm $Image -- true }
        $exoStats = Convert-SamplesToStats $exoSamples
        Write-Host "Exo spawn median:    $($exoStats.median_ms) ms (min=$($exoStats.min_ms), p95=$($exoStats.p95_ms), max=$($exoStats.max_ms))"
    } catch {
        Write-Warning "Exo benchmark failed: $($_.Exception.Message)"
    }
} else {
    Write-Warning "Exo binary not found: $ExoBin"
}

$volumeResult = $null
if ($IncludeVolume) {
    Write-Host "==> Volume mount benchmark"
    $volumeDir = Join-Path $OutDir "bench-volume"
    New-Item -ItemType Directory -Force -Path $volumeDir | Out-Null
    Set-Content -Encoding UTF8 -Path (Join-Path $volumeDir "bench.txt") -Value "exo-bench-volume"
    $volumeHost = (Resolve-Path $volumeDir).Path
    $volumeWsl = Convert-ToWslPath $volumeHost
    $dockerMountHost = if ($DockerViaWsl) { $volumeWsl } else { $volumeHost }
    $dockerMount = "${dockerMountHost}:/workspace:ro"
    $exoMount = "${volumeWsl}:/workspace"

    $dockerVolumeStats = $null
    $exoVolumeStats = $null
    try {
        $dockerVolumeSamples = Measure-SamplesMs -Count $Runs -Script { Invoke-DockerQuiet @("run", "--rm", "-v", $dockerMount, $Image, "sh", "-c", "test -f /workspace/bench.txt") }
        $dockerVolumeStats = Convert-SamplesToStats $dockerVolumeSamples
        Write-Host "Docker volume median: $($dockerVolumeStats.median_ms) ms (min=$($dockerVolumeStats.min_ms), p95=$($dockerVolumeStats.p95_ms), max=$($dockerVolumeStats.max_ms))"
    } catch {
        Write-Warning "Docker volume benchmark failed: $($_.Exception.Message)"
    }

    if ($exoAvailable) {
        try {
            $exoVolumeSamples = Measure-SamplesMs -Count $Runs -Script { & $ExoBin run --rm -v $exoMount $Image -- sh -c "test -f /workspace/bench.txt" }
            $exoVolumeStats = Convert-SamplesToStats $exoVolumeSamples
            Write-Host "Exo volume median:    $($exoVolumeStats.median_ms) ms (min=$($exoVolumeStats.min_ms), p95=$($exoVolumeStats.p95_ms), max=$($exoVolumeStats.max_ms))"
        } catch {
            Write-Warning "Exo volume benchmark failed: $($_.Exception.Message)"
        }
    }

    $volumeResult = [ordered]@{
        host_path = $volumeHost
        exo_path = $volumeWsl
        docker = $dockerVolumeStats
        exo = $exoVolumeStats
    }
}

$firstRunResult = $null
if ($IncludeFirstRun) {
    Write-Host "==> Pull + first-run benchmark"
    if (-not $ColdImage) { $ColdImage = $Image }

    $coldDelete = [bool]$AllowColdDelete
    if ($coldDelete) {
        if ($ColdImage -eq $Image) {
            Write-Warning "Cold delete will remove the same image used by the spawn benchmark ($Image). Pass a distinct -ColdImage to avoid re-pull side effects."
        }
        Write-Host "    (cold mode: removing '$ColdImage' from Docker and Exo before timing)"
        # Targeted, explicit image removal only -- no recursive path deletion and
        # no PID killing. Both rmi calls are scoped to the named ColdImage.
        try { Invoke-DockerQuiet @("rmi", "-f", $ColdImage) } catch { Write-Warning "Docker rmi skipped: $($_.Exception.Message)" }
        if ($exoAvailable) {
            try { Invoke-NativeQuiet { & $ExoBin rmi $ColdImage } } catch { Write-Warning "Exo rmi skipped: $($_.Exception.Message)" }
        }
    }

    $dockerPullMs = $null
    $dockerFirstRunMs = $null
    $exoPullMs = $null
    $exoFirstRunMs = $null

    try {
        $dockerPullMs = Measure-OneMs { Invoke-DockerQuiet @("pull", $ColdImage) }
        $dockerFirstRunMs = Measure-OneMs { Invoke-DockerQuiet @("run", "--rm", $ColdImage, "true") }
        Write-Host "Docker pull/ensure: ${dockerPullMs} ms; first run after ensure: ${dockerFirstRunMs} ms"
    } catch {
        Write-Warning "Docker pull/first-run benchmark failed: $($_.Exception.Message)"
    }

    if ($exoAvailable) {
        try {
            $exoPullMs = Measure-OneMs { & $ExoBin pull $ColdImage }
            $exoFirstRunMs = Measure-OneMs { & $ExoBin run --rm $ColdImage -- true }
            Write-Host "Exo pull/ensure:    ${exoPullMs} ms; first run after ensure: ${exoFirstRunMs} ms"
        } catch {
            Write-Warning "Exo pull/first-run benchmark failed: $($_.Exception.Message)"
        }
    }

    $firstRunNote = if ($coldDelete) {
        "Cold pull: image was removed from both runtimes before timing, so pull_or_ensure_ms reflects a real download/extract."
    } else {
        "Non-destructive pull/ensure timing. If the image was already present, this is warm ensure, not a true cold pull."
    }

    $firstRunResult = [ordered]@{
        image = $ColdImage
        cold = $coldDelete
        note = $firstRunNote
        docker = [ordered]@{ pull_or_ensure_ms = $dockerPullMs; first_run_after_ensure_ms = $dockerFirstRunMs }
        exo = [ordered]@{ pull_or_ensure_ms = $exoPullMs; first_run_after_ensure_ms = $exoFirstRunMs }
    }
}

$agentImageResults = @()
if ($IncludeAgentImages) {
    Write-Host "==> Docker agent image startup benchmark"
    foreach ($agentImage in $AgentImages) {
        $bytes = Get-DockerImageBytesOrNull $agentImage
        if ($null -eq $bytes) {
            Write-Warning "Agent image not found in Docker: $agentImage"
            $agentImageResults += [ordered]@{ image = $agentImage; found = $false }
            continue
        }

        $helpStats = $null
        $lifecycleStats = $null
        try {
            $helpStats = Convert-SamplesToStats (Measure-SamplesMs -Count $Runs -Script { Invoke-DockerQuiet @("run", "--rm", $agentImage, "--help") })
            $lifecycleStats = Convert-SamplesToStats (Measure-SamplesMs -Count $Runs -Script { Invoke-DockerQuiet @("run", "--rm", $agentImage) })
            Write-Host "$agentImage help median: $($helpStats.median_ms) ms; lifecycle median: $($lifecycleStats.median_ms) ms; size: $([math]::Round($bytes / 1MB, 1)) MiB"
        } catch {
            Write-Warning "Agent image benchmark failed for ${agentImage}: $($_.Exception.Message)"
        }

        $agentImageResults += [ordered]@{
            image = $agentImage
            found = $true
            image_bytes = $bytes
            docker_help = $helpStats
            docker_closed_stdin_lifecycle = $lifecycleStats
        }
    }
}

$daemonResult = $null
if ($IncludeDaemon) {
    Write-Host "==> Exo daemon benchmark/status"
    $statusText = $null
    $running = $false
    if ($exoAvailable) {
        try {
            $statusText = Invoke-NativeCapture { & $ExoBin daemon --status --json }
            $running = ($statusText -match '"running"\s*:\s*true')
            Write-Host "Exo daemon running: $running"
        } catch {
            Write-Warning "Could not read Exo daemon status: $($_.Exception.Message)"
        }
    }

    # Safety note: the Linux exo daemon command currently runs in the foreground
    # even without --foreground when invoked through exo.cmd, so this script does
    # not auto-start or auto-stop it. If a daemon is already running, measure the
    # detached start path; otherwise record the skipped status.
    $daemonDetachStats = $null
    if ($running) {
        try {
            $daemonDetachStats = Convert-SamplesToStats (Measure-SamplesMs -Count $Runs -Script { & $ExoBin run --rm -d $Image -- true })
            Write-Host "Exo daemon detached median: $($daemonDetachStats.median_ms) ms"
        } catch {
            Write-Warning "Exo daemon detached benchmark failed: $($_.Exception.Message)"
        }
    }

    $daemonResult = [ordered]@{
        status_raw = $statusText
        running = $running
        detached_start = $daemonDetachStats
        note = "Daemon is not auto-started/stopped by this script to avoid foreground hangs or killing runtime-owned processes."
    }
}

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$out = Join-Path $OutDir "bench-windows-$stamp.json"
$result = [ordered]@{
    stamp = $stamp
    runs = $Runs
    image = $Image
    docker = [ordered]@{
        via_wsl = [bool]$DockerViaWsl
        spawn = $dockerStats
        image_bytes = $dockerImageBytes
    }
    exo = [ordered]@{
        bin = $ExoBin
        spawn = $exoStats
    }
    volume = $volumeResult
    first_run = $firstRunResult
    agent_images = $agentImageResults
    daemon = $daemonResult
}
$result | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 $out
Write-Host "Wrote $out"

if ($EmitMarkdown) {
    $converter = Join-Path $PSScriptRoot "bench-to-markdown.ps1"
    if (Test-Path $converter) {
        $md = [System.IO.Path]::ChangeExtension($out, ".md")
        & $converter -JsonPath $out -OutFile $md
        Write-Host "Wrote $md"
    } else {
        Write-Warning "Markdown converter not found: $converter"
    }
}
