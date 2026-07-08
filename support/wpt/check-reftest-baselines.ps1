[CmdletBinding()]
param(
    [switch]$NoBuild
)

# Reftest baselines are a LOCAL guard, not default CI: reftests render through
# the GPU (`Renderer::boot`), which the headless CI runner does not have. Run
# this locally to confirm `unexpected=0` on the checked reftest slices.

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

if (-not $NoBuild) {
    cargo build --manifest-path (Join-Path $repo "Cargo.toml") --release -p serval-wpt
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build -p serval-wpt --release failed with exit code $LASTEXITCODE"
    }
}

$metadata = cargo metadata --manifest-path (Join-Path $repo "Cargo.toml") --format-version 1 --no-deps | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) {
    throw "cargo metadata failed with exit code $LASTEXITCODE"
}

$exeName = if ($IsWindows -or $env:OS -eq "Windows_NT") { "serval-wpt.exe" } else { "serval-wpt" }
$runner = Join-Path $metadata.target_directory (Join-Path "release" $exeName)
if (-not (Test-Path $runner)) {
    throw "release serval-wpt binary not found at $runner; rerun without -NoBuild"
}

$baselines = @(
    @{
        Subset = "css/mediaqueries"
        Engine = "boa"
        Expectations = "ports/serval-wpt/expectations/reftest/css_mediaqueries_boa.json"
    }
)

foreach ($baseline in $baselines) {
    $expectations = Join-Path $repo $baseline.Expectations
    Write-Output "Checking WPT reftest baseline: $($baseline.Subset) [$($baseline.Engine)]"
    & $runner reftest $baseline.Subset --engine $baseline.Engine --expectations $expectations
    if ($LASTEXITCODE -ne 0) {
        throw "WPT reftest baseline failed: $($baseline.Subset) [$($baseline.Engine)]"
    }
}

Write-Output "WPT reftest baselines: unexpected=0"
