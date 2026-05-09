# Source this in a PowerShell session to set the env that makes
# `cargo check -p servo-layout` (and `cargo check -p servo`) build
# under VS 2022. Required because:
#  - mozjs_sys 0.140.10-2 has unprotected GCC syntax (`__attribute__((__packed__))`)
#    in headers, so MSVC `cl.exe` cannot compile it — need clang-cl.
#  - mozjs vendored fmt 11.x requires `/utf-8` flag (or `-utf-8` for msys2-safety).
#  - aws-lc-sys needs NASM on PATH.
#  - mozjs_sys looks for moztools/MozillaBuild via env or known paths.
#
# Usage:
#   . .\.cargo-check-logs\cargo-check-env.ps1
#   cmd /c '"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" 1>nul 2>&1 && cargo check -p servo-layout'

$env:Path = "C:\Users\mark_\AppData\Local\bin\NASM;$env:Path"
$env:CFLAGS = "-utf-8"
$env:CXXFLAGS = "-utf-8"
$env:CC = "clang-cl"
$env:CXX = "clang-cl"
$env:HOST_CC = "clang-cl"
$env:HOST_CXX = "clang-cl"
$env:MOZILLABUILD = "C:/mozilla-build"

Write-Host "C3 build env loaded (clang-cl, -utf-8, NASM, MOZILLABUILD)."
Write-Host "Run cargo via:  cmd /c '`"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat`" 1>nul 2>&1 && cargo check -p servo-layout'"
