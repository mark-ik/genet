#!/usr/bin/env bash
#
# Profile-ladder tier gate (cross-platform; runs locally and in CI).
#
# The JS-engine-free tiers of the profile ladder
# (docs/2026-05-12_serval_profile_ladder_plan.md) must not pull a script engine,
# a heavy servo render/host subsystem, or the WebGL shader compiler. This is the
# package-witness check the ladder rests on: a low tier that accidentally depends
# on `script-engine-*` / `mozjs` / `servo-media` / etc. has silently broken the
# tier contract. Supersedes the Windows-only single-tier check-static-html.ps1.
#
# `cargo tree` only RESOLVES the dependency graph (no build), so this is cheap.
# Crate names are matched exactly (anchored), so "boa" does not match "keyboard".

set -euo pipefail

# Forbidden crate names (exact). Extend as new heavy/engine crates appear.
BLOCKED='^(boa|boa_engine|nova|nova_vm|script-engine-api|script-engine-boa|script-engine-nova|script-runtime-api|mozjs|mozjs-sys|servo-script|servo-script-bindings|servo-media|servo-media-thread|servo-storage|servo-storage-traits|regress|webgl-essl|servo-webgl-essl)$'

# Tiers that must stay clean (the static, no-JS lanes).
TIERS=(serval-static-dom serval-static-html)

fail=0
for tier in "${TIERS[@]}"; do
  hits="$(cargo tree -p "$tier" --prefix none -f '{p}' 2>/dev/null \
            | awk '{print $1}' | sort -u | grep -E "$BLOCKED" || true)"
  if [ -n "$hits" ]; then
    echo "FAIL: tier '$tier' pulled forbidden crates:" >&2
    echo "$hits" | sed 's/^/    /' >&2
    fail=1
  else
    echo "ok: $tier is clean (no JS engine, no heavy subsystem)"
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "" >&2
  echo "Profile-ladder tier gate FAILED: a JS-free tier gained a forbidden dependency." >&2
  exit 1
fi
echo "profile-ladder tier gate passed"
