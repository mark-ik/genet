[CmdletBinding()]
param(
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

if (-not $NoBuild) {
    cargo build --manifest-path (Join-Path $repo "Cargo.toml") --release -p genet-wpt --features netfetch
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build -p genet-wpt --release --features netfetch failed with exit code $LASTEXITCODE"
    }
}

$metadata = cargo metadata --manifest-path (Join-Path $repo "Cargo.toml") --format-version 1 --no-deps | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}

$exeName = if ($IsWindows -or $env:OS -eq "Windows_NT") { "genet-wpt.exe" } else { "genet-wpt" }
$runner = Join-Path $metadata.target_directory (Join-Path "release" $exeName)
if (-not (Test-Path $runner)) {
    throw "release genet-wpt binary not found at $runner; rerun without -NoBuild"
}

$baselines = @(
    @{
        Subset = "fetch/api/basic"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/fetch_api_basic_boa.json"
    }
)

foreach ($baseline in $baselines) {
    $expectations = Join-Path $repo $baseline.Expectations
    Write-Output "Checking WPT fetch testharness baseline: $($baseline.Subset) [$($baseline.Engine)]"
    & $runner testharness $baseline.Subset --spawn-server --engine $baseline.Engine --expectations $expectations
    if ($LASTEXITCODE -ne 0) {
        throw "WPT fetch testharness baseline failed: $($baseline.Subset) [$($baseline.Engine)]"
    }
}

Write-Output "WPT fetch testharness baselines: unexpected=0"
