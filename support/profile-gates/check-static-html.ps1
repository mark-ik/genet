param(
    [switch]$SkipCargoCheck
)

$ErrorActionPreference = "Stop"

if (-not $SkipCargoCheck) {
    cargo check -p serval-static-html
}

$tree = cargo tree -p serval-static-html
$blocked = $tree | Select-String -Pattern "servo-script|servo-script-bindings|mozjs|servo-media|servo-storage"

if ($blocked) {
    Write-Error ("serval-static-html pulled blocked dependencies:`n" + ($blocked -join "`n"))
}

Write-Host "serval-static-html dependency gate passed"
