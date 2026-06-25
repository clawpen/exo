param(
    [ValidateSet("standard", "slim")]
    [string]$Variant = "standard",

    [string]$Tag = "exo-agent:docker-test",

    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

docker info *> $null

$dockerfile = if ($Variant -eq "slim") {
    "images/exo-agent/Containerfile.slim"
} else {
    "images/exo-agent/Containerfile"
}

if (-not $SkipBuild) {
    docker build -t $Tag -f $dockerfile .
}

Write-Host "==> CLI help smoke test"
docker run --rm $Tag --help | Out-Host

Write-Host "==> EOF/stdin lifecycle smoke test"
# With stdin closed, the agent initializes config + SQLite memory, observes EOF,
# and exits without making an LLM API call.
docker run --rm $Tag | Out-Host

Write-Host "Docker smoke tests passed for $Tag ($Variant)."
