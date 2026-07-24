#!/usr/bin/env bash
# F3 fidelity ledger: run both renderers over the same WPT directories.
# Sequential by design (no concurrent test runs).
set -u
cd "$(dirname "$0")/../.." || exit 1
SP="${LEDGER_OUT:-./target/ledger}"
mkdir -p "$SP"
BIN=./target/release/genet-wpt

DIRS="
css/CSS2
css/css-align
css/css-backgrounds
css/css-borders
css/css-box
css/css-cascade
css/css-color
css/css-display
css/css-flexbox
css/css-fonts
css/css-grid
css/css-images
css/css-multicol
css/css-overflow
css/css-position
css/css-pseudo
css/css-sizing
css/css-tables
css/css-text
css/css-transitions
css/css-ui
css/css-values
css/css-variables
css/css-writing-modes
css/cssom
css/cssom-view
css/selectors
"

for d in $DIRS; do
  slug=$(echo "$d" | tr '/' '_')
  for r in stylo livery; do
    out="$SP/${slug}_${r}.json"
    [ -s "$out" ] && { echo "SKIP $d [$r] (have)"; continue; }
    echo "=== $d [$r]"
    timeout 1800 "$BIN" testharness "$d" --renderer "$r" --write-expectations "$out" 2>&1 | tail -1
  done
done
echo "LEDGER RUNS COMPLETE"
