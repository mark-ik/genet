[CmdletBinding()]
param(
    [string]$Wasm64Toolchain = "nightly-2026-06-22",
    [string]$StableToolchain = "1.95.0",
    [string]$OutputDirectory = "dist/scripted-workers"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo $OutputDirectory
$metadata = cargo metadata --manifest-path (Join-Path $repo "Cargo.toml") --format-version 1 --no-deps | ConvertFrom-Json
$target = $metadata.target_directory

$wasmBindgen = Get-Command wasm-bindgen -ErrorAction SilentlyContinue
if (-not $wasmBindgen) {
    throw "wasm-bindgen CLI 0.2.126 is required: cargo install wasm-bindgen-cli --version 0.2.126 --locked"
}
$version = & $wasmBindgen.Source --version
if ($version -notmatch "0\.2\.126$") {
    throw "wasm-bindgen CLI must be 0.2.126; found: $version"
}

rustup run $Wasm64Toolchain rustc --version | Out-Null
rustup component add rust-src --toolchain $Wasm64Toolchain | Out-Null

$previousRustFlags = $env:RUSTFLAGS
try {
    $env:RUSTFLAGS = '--cfg getrandom_backend="wasm_js"'
    rustup run $Wasm64Toolchain cargo build `
        --manifest-path (Join-Path $repo "Cargo.toml") `
        --release `
        --package genet-scripted-worker `
        --no-default-features `
        --features engine-nova `
        --target wasm64-unknown-unknown `
        -Z build-std=std,panic_abort
} finally {
    $env:RUSTFLAGS = $previousRustFlags
}

rustup run $StableToolchain cargo build `
    --manifest-path (Join-Path $repo "Cargo.toml") `
    --release `
    --package genet-scripted-worker `
    --no-default-features `
    --features engine-boa `
    --target wasm32-unknown-unknown

$novaOut = Join-Path $out "nova"
$boaOut = Join-Path $out "boa"
New-Item -ItemType Directory -Force -Path $novaOut | Out-Null
New-Item -ItemType Directory -Force -Path $boaOut | Out-Null

& $wasmBindgen.Source `
    (Join-Path $target "wasm64-unknown-unknown/release/genet_scripted_worker.wasm") `
    --target web `
    --out-dir $novaOut `
    --out-name genet-scripted-nova-wasm64

& $wasmBindgen.Source `
    (Join-Path $target "wasm32-unknown-unknown/release/genet_scripted_worker.wasm") `
    --target web `
    --out-dir $boaOut `
    --out-name genet-scripted-boa-wasm32

Copy-Item (Join-Path $repo "components/genet-scripted-worker/loader.mjs") $out -Force
Copy-Item (Join-Path $repo "components/genet-scripted-worker/worker-bootstrap.mjs") $out -Force

Write-Output "Generated genet-scripted-nova-wasm64 and genet-scripted-boa-wasm32 in $out"
