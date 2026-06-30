[CmdletBinding()]
param(
    [switch]$NoBuild
)

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
        Subset = "dom"
        Engine = "boa"
        Expectations = "ports/serval-wpt/expectations/testharness/dom_boa.json"
    },
    @{
        Subset = "dom/abort"
        Engine = "boa"
        Expectations = "ports/serval-wpt/expectations/testharness/dom_abort_boa.json"
    },
    @{
        Subset = "dom/nodes"
        Engine = "boa"
        Expectations = "ports/serval-wpt/expectations/testharness/dom_nodes_boa.json"
    },
    @{
        Subset = "html/webappapis/timers"
        Engine = "boa"
        Expectations = "ports/serval-wpt/expectations/testharness/html_webappapis_timers_boa.json"
    }
)

foreach ($baseline in $baselines) {
    $expectations = Join-Path $repo $baseline.Expectations
    Write-Output "Checking WPT testharness baseline: $($baseline.Subset) [$($baseline.Engine)]"
    & $runner testharness $baseline.Subset --engine $baseline.Engine --expectations $expectations
    if ($LASTEXITCODE -ne 0) {
        throw "WPT testharness baseline failed: $($baseline.Subset) [$($baseline.Engine)]"
    }
}

Write-Output "WPT testharness baselines: unexpected=0"
