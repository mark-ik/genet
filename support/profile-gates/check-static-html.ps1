param(
    [switch]$SkipCargoCheck
)

$ErrorActionPreference = "Stop"

if (-not $SkipCargoCheck) {
    cargo check -p genet-static-html
}

$tree = cargo tree -p genet-static-html
$blocked = $tree | Select-String -Pattern "servo-script|servo-script-bindings|mozjs|servo-media|servo-storage"

if ($blocked) {
    Write-Error ("genet-static-html pulled blocked dependencies:`n" + ($blocked -join "`n"))
}

Write-Host "genet-static-html dependency gate passed"
