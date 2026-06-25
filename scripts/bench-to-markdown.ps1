<#
.SYNOPSIS
    Convert one or more bench result JSON files into a Markdown report.
    Handles both `bench-vs-docker.ps1` JSON files and `bench-density.sh` JSON files.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts/bench-to-markdown.ps1 `
        -JsonPath results/bench-windows-20260623-233017.json -OutFile results/summary.md

.EXAMPLE
    # Combine several runs into one Markdown report
    powershell -ExecutionPolicy Bypass -File scripts/bench-to-markdown.ps1 `
        -JsonPath (Get-ChildItem results/bench-*.json | Select-Object -Expand FullName)
#>
param(
    [Parameter(Mandatory = $true)]
    [string[]]$JsonPath,

    [string]$OutFile
)

$ErrorActionPreference = "Stop"

function Format-Stat {
    param($Stat)
    if ($null -eq $Stat) { return "n/a" }
    return ("{0} ms (min {1} / p95 {2} / max {3})" -f $Stat.median_ms, $Stat.min_ms, $Stat.p95_ms, $Stat.max_ms)
}

function Get-FasterText {
    param($DockerMs, $ExoMs)
    if ($null -eq $DockerMs -or $null -eq $ExoMs -or $DockerMs -eq 0) { return "n/a" }
    $pct = [math]::Round(((($DockerMs - $ExoMs) / $DockerMs) * 100), 1)
    if ($pct -ge 0) { return "Exo $pct% faster" }
    return "Exo $([math]::Abs($pct))% slower"
}

function Format-Mib {
    param($Bytes)
    if ($null -eq $Bytes) { return "n/a" }
    return ("{0} MiB" -f [math]::Round($Bytes / 1MB, 1))
}

$results = @()
foreach ($path in $JsonPath) {
    $resolved = (Resolve-Path $path).Path
    $json = Get-Content -Raw $resolved | ConvertFrom-Json
    $results += [pscustomobject]@{ File = (Split-Path $resolved -Leaf); Data = $json }
}

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("# Exo vs Docker benchmark report")
$lines.Add("")
$lines.Add("_Generated: $(Get-Date -Format 'yyyy-MM-dd HH:mm') (local)._")
$lines.Add("")
$lines.Add("Spawn/volume timings are wall-clock medians of the configured run count. Positive percentages mean Exo is faster than Docker.")
$lines.Add("")

# Combined spawn summary
$lines.Add("## Spawn latency (summary)")
$lines.Add("")
$lines.Add("| Image | Runs | Docker median | Exo median | Delta (ms) | Result |")
$lines.Add("| --- | ---: | ---: | ---: | ---: | --- |")
foreach ($r in $results) {
    $d = $r.Data
    $dm = $d.docker.spawn.median_ms
    $em = $d.exo.spawn.median_ms
    $delta = if ($null -ne $dm -and $null -ne $em) { $dm - $em } else { "n/a" }
    $faster = Get-FasterText $dm $em
    $lines.Add(("| ``{0}`` | {1} | {2} ms | {3} ms | {4} | {5} |" -f $d.image, $d.runs, $dm, $em, $delta, $faster))
}
$lines.Add("")

# Combined volume summary (only rows with volume data)
$hasVolume = $results | Where-Object { $_.Data.volume -and $_.Data.volume.docker -and $_.Data.volume.exo }
if ($hasVolume) {
    $lines.Add("## Volume mount latency (summary)")
    $lines.Add("")
    $lines.Add("| Image | Runs | Docker median | Exo median | Delta (ms) | Result |")
    $lines.Add("| --- | ---: | ---: | ---: | ---: | --- |")
    foreach ($r in $hasVolume) {
        $d = $r.Data
        $dm = $d.volume.docker.median_ms
        $em = $d.volume.exo.median_ms
        $delta = if ($null -ne $dm -and $null -ne $em) { $dm - $em } else { "n/a" }
        $faster = Get-FasterText $dm $em
        $lines.Add(("| ``{0}`` | {1} | {2} ms | {3} ms | {4} | {5} |" -f $d.image, $d.runs, $dm, $em, $delta, $faster))
    }
    $lines.Add("")
}

# Density summary (only rows with density data)
$hasDensity = $results | Where-Object { $_.Data.density }
if ($hasDensity) {
    $lines.Add("## Density (containers per GB RAM)")
    $lines.Add("")
    $lines.Add("| Image | Max containers | Failure point | RSS/container | Host used RSS |")
    $lines.Add("| --- | ---: | ---: | ---: | ---: |")
    foreach ($r in $hasDensity) {
        $d = $r.Data
        $dns = $d.density
        $rss = if ($dns.rss_per_container_kb) { "$($dns.rss_per_container_kb) KiB" } else { "n/a" }
        $host = if ($dns.host_used_rss_mb) { "$($dns.host_used_rss_mb) MiB" } else { "n/a" }
        $lines.Add(("| ``{0}`` | {1} | {2} | {3} | {4} |" -f $d.image, $dns.max_containers, $dns.failure_point, $rss, $host))
    }
    if ($hasDensity[0].Data.docker -and $hasDensity[0].Data.docker.max_containers) {
        $lines.Add("")
        $lines.Add("### Docker density comparison")
        $lines.Add("")
        $lines.Add("| Image | Max containers | RSS/container |")
        $lines.Add("| --- | ---: | ---: |")
        foreach ($r in $hasDensity) {
            $d = $r.Data
            if ($d.docker -and $d.docker.max_containers) {
                $rss = if ($d.docker.per_container_kb) { "$($d.docker.per_container_kb) KiB" } else { "n/a" }
                $lines.Add(("| ``{0}`` | {1} | {2} |" -f $d.image, $d.docker.max_containers, $rss))
            }
        }
    }
    $lines.Add("")
}

# OpenClaw agent startup summary (only rows with openclaw data)
$hasOpenClaw = $results | Where-Object { $_.Data.startup_ms }
if ($hasOpenClaw) {
    $lines.Add("## OpenClaw agent startup")
    $lines.Add("")
    $lines.Add("| Image | Runs | Docker median | Exo median | Delta (ms) | Result |")
    $lines.Add("| --- | ---: | ---: | ---: | ---: | --- |")
    foreach ($r in $hasOpenClaw) {
        $d = $r.Data
        $dm = $d.startup_ms.docker
        $em = $d.startup_ms.exo
        $delta = if ($null -ne $dm -and $null -ne $em -and $dm -gt 0) { $dm - $em } else { "n/a" }
        $faster = Get-FasterText $dm $em
        $lines.Add(("| ``{0}`` | {1} | {2} ms | {3} ms | {4} | {5} |" -f $d.image, $d.runs, $dm, $em, $delta, $faster))
    }
    $lines.Add("")
}

# Per-file detail
$lines.Add("## Per-run detail")
$lines.Add("")
foreach ($r in $results) {
    $d = $r.Data
    $lines.Add("### ``$($d.image)`` ($($r.File))")
    $lines.Add("")
    $lines.Add("| Metric | Docker | Exo | Result |")
    $lines.Add("| --- | --- | --- | --- |")
    $lines.Add(("| Spawn | {0} | {1} | {2} |" -f (Format-Stat $d.docker.spawn), (Format-Stat $d.exo.spawn), (Get-FasterText $d.docker.spawn.median_ms $d.exo.spawn.median_ms)))
    if ($d.volume -and $d.volume.docker -and $d.volume.exo) {
        $lines.Add(("| Volume mount | {0} | {1} | {2} |" -f (Format-Stat $d.volume.docker), (Format-Stat $d.volume.exo), (Get-FasterText $d.volume.docker.median_ms $d.volume.exo.median_ms)))
    }
    $lines.Add(("| Image size (Docker) | {0} | - | - |" -f (Format-Mib $d.docker.image_bytes)))
    $lines.Add("")

    if ($d.first_run) {
        $fr = $d.first_run
        $mode = if ($fr.cold) { "cold (image removed first)" } else { "warm ensure" }
        $lines.Add("**Pull / first run** ($mode) for ``$($fr.image)``:")
        $lines.Add("")
        $lines.Add("| Phase | Docker | Exo |")
        $lines.Add("| --- | ---: | ---: |")
        $dp = if ($null -ne $fr.docker.pull_or_ensure_ms) { "$($fr.docker.pull_or_ensure_ms) ms" } else { "n/a" }
        $ep = if ($null -ne $fr.exo.pull_or_ensure_ms) { "$($fr.exo.pull_or_ensure_ms) ms" } else { "n/a" }
        $dfr = if ($null -ne $fr.docker.first_run_after_ensure_ms) { "$($fr.docker.first_run_after_ensure_ms) ms" } else { "n/a" }
        $efr = if ($null -ne $fr.exo.first_run_after_ensure_ms) { "$($fr.exo.first_run_after_ensure_ms) ms" } else { "n/a" }
        $lines.Add(("| Pull / ensure | {0} | {1} |" -f $dp, $ep))
        $lines.Add(("| First run after ensure | {0} | {1} |" -f $dfr, $efr))
        $lines.Add("")
        $lines.Add("> $($fr.note)")
        $lines.Add("")
    }

    if ($d.agent_images -and $d.agent_images.Count -gt 0) {
        $lines.Add("**Docker agent image startup:**")
        $lines.Add("")
        $lines.Add("| Image | Size | --help median | Closed-stdin lifecycle median |")
        $lines.Add("| --- | ---: | ---: | ---: |")
        foreach ($ai in $d.agent_images) {
            if (-not $ai.found) {
                $lines.Add(("| ``{0}`` | not found | - | - |" -f $ai.image))
                continue
            }
            $help = if ($ai.docker_help) { "$($ai.docker_help.median_ms) ms" } else { "n/a" }
            $life = if ($ai.docker_closed_stdin_lifecycle) { "$($ai.docker_closed_stdin_lifecycle.median_ms) ms" } else { "n/a" }
            $lines.Add(("| ``{0}`` | {1} | {2} | {3} |" -f $ai.image, (Format-Mib $ai.image_bytes), $help, $life))
        }
        $lines.Add("")
    }

    if ($d.daemon) {
        $lines.Add("**Exo daemon:** running=$($d.daemon.running)")
        if ($d.daemon.detached_start) {
            $lines.Add("")
            $lines.Add("- Detached start median: $($d.daemon.detached_start.median_ms) ms")
        }
        $lines.Add("")
        $lines.Add("> $($d.daemon.note)")
        $lines.Add("")
    }
}

$markdown = ($lines -join "`n")

if ($OutFile) {
    Set-Content -Encoding UTF8 -Path $OutFile -Value $markdown
    Write-Host "Wrote $OutFile"
} else {
    Write-Output $markdown
}
