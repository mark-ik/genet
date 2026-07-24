[CmdletBinding()]
param(
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

if (-not $NoBuild) {
    cargo build --manifest-path (Join-Path $repo "Cargo.toml") --release -p genet-wpt
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build -p genet-wpt --release failed with exit code $LASTEXITCODE"
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
        Subset = "dom"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/dom_boa.json"
    },
    @{
        Subset = "dom/abort"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/dom_abort_boa.json"
    },
    @{
        Subset = "dom/nodes"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/dom_nodes_boa.json"
    },
    @{
        Subset = "html/webappapis/timers"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/html_webappapis_timers_boa.json"
    },
    @{
        # matchMedia over a default device (no GPU / render needed).
        Subset = "css/mediaqueries"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/css_mediaqueries_boa.json"
    },
    @{
        Subset = "css/css-position"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/css_position_boa.json"
    },
    @{
        # `@keyframes` parsing + computed values. The event-order and
        # interpolation-over-time tests in this corpus cannot pass yet: the
        # testharness lane drives no layout session, so it has no animation clock
        # and no rAF pump (see the CSS animations plan, A3). Pinned so the corpus
        # moves visibly when that lands.
        Subset = "css/css-animations"
        Engine = "boa"
        Expectations = "ports/genet-wpt/expectations/testharness/css_animations_boa.json"
    },
    @{
        # The Livery style route (harvest plan, H5 tree-counting). Only the
        # parsing file passes fully today; the rest need keyframe re-resolution,
        # shadow DOM, layout geometry, and registered properties, so this is
        # pinned to move visibly as those slices land. Baselines record their
        # renderer and the runner refuses a mismatched --renderer, so a Livery
        # baseline cannot silently vouch for a Stylo run.
        Subset = "css/css-values/tree-counting"
        Engine = "boa"
        Renderer = "livery"
        Expectations = "ports/genet-wpt/expectations/testharness/css_values_tree_counting_livery_boa.json"
    }
)

# The Livery advanced-math computed-value files (harvest plan, H5). Baselines
# pin exact subtest counts (subtests_passed/subtests_total per file), so a
# regression inside a still-failing file is unexpected, and a progression is
# too -- re-pin with --write-expectations so the baseline ratchets upward.
# Every math slice (ex/ch, NaN) reruns and updates these.
$liveryMathFiles = @(
    "round-mod-rem-computed", "hypot-pow-sqrt-computed", "sin-cos-tan-computed",
    "acos-asin-atan-atan2-computed", "exp-log-compute"
)
foreach ($file in $liveryMathFiles) {
    $baselines += @{
        Subset       = "css/css-values/$file.html"
        Engine       = "boa"
        Renderer     = "livery"
        Expectations = "ports/genet-wpt/expectations/testharness/css_values_$($file -replace '-','_')_livery_boa.json"
    }
}

foreach ($baseline in $baselines) {
    $expectations = Join-Path $repo $baseline.Expectations
    $renderer = $baseline.Renderer
    $label = if ($renderer) { "$($baseline.Subset) [$($baseline.Engine)/$renderer]" } else { "$($baseline.Subset) [$($baseline.Engine)]" }
    Write-Output "Checking WPT testharness baseline: $label"
    $extra = if ($renderer) { @("--renderer", $renderer) } else { @() }
    & $runner testharness $baseline.Subset --engine $baseline.Engine @extra --expectations $expectations
    if ($LASTEXITCODE -ne 0) {
        throw "WPT testharness baseline failed: $label"
    }
}

Write-Output "WPT testharness baselines: unexpected=0"
