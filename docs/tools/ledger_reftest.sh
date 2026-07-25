#!/usr/bin/env bash
# F3b fidelity ledger, reftest lane: layout and paint, where testharness is blind.
# Sequential by design (no concurrent test runs). Needs a GPU.
set -u
cd "$(dirname "$0")/../.." || exit 1
SP="${LEDGER_OUT:-./target/ledger-reftest}"
mkdir -p "$SP"
BIN=./target/release/genet-wpt

# Priority order: the directories the testharness lane could not measure
# (high skip rates there) come first, so the gate answers early.
DIRS="
css/css-multicol
css/css-tables
css/css-borders
css/css-writing-modes
css/css-flexbox
css/css-position
css/css-backgrounds
css/css-grid
css/CSS2
"

for d in $DIRS; do
  slug=$(echo "$d" | tr '/' '_')
  for r in stylo livery; do
    out="$SP/${slug}_${r}.json"
    [ -s "$out" ] && { echo "SKIP $d [$r] (have)"; continue; }
    echo "=== $d [$r]"
    "$BIN" reftest "$d" --renderer "$r" --write-expectations "$out" 2>&1 | tail -2
  done
done
echo "REFTEST LEDGER COMPLETE"
