# Absolute CSS conformance ledger

**Status:** implemented on `codex/absolute-css-conformance-ledger`

**Authority:** [Livery fullweb cutover and servo retirement](2026-07-24_livery_fullweb_cutover_and_servo_retirement_plan.md)

## Purpose

Measure Genet against WPT itself. The Stylo differential remains the F4
replacement-safety instrument. This ledger never reads a Stylo result and
cannot be used to claim parity.

The ledger joins two exact `genet-wpt` result maps to the authoritative WPT
manifest, but keeps their capability routes distinct:

- one screen `reftest` map whose style, layout, and rendering route is Livery;
- one `testharness` map whose CSSOM and computed-style route is Livery while
  Stylo still drives geometry and animation; and
- every manifest test under the selected scope, including tests absent from
  either result map.

The output reports absolute passing subtests and files. It does not report a
percentage. The testharness total is not presented as pure Livery layout
conformance until its geometry route actually moves to Livery.

## What stays visible

For each runnable lane the report distinguishes:

- all manifest tests assigned to the lane;
- manifest tests the current window-shaped runner cannot host;
- observed tests and their pass, fail, skip, error, or no-results status;
- hostable manifest tests missing from the supplied result maps; and
- testharness files that produced no subtest counts.

Print reftests remain unsupported because the runner has only a fixed screen
route. Manual, visual, WebDriver, and crash-only manifest kinds remain in the
same separate unsupported count. They do not disappear into a denominator
chosen after the run.

By default, `genet-wpt conformance` rejects a report if any hostable reftest or
testharness test is absent. `--allow-incomplete-conformance` exists only for
diagnosing partial result sets. Result maps are pinned to the selected subset,
the SHA-256 of `MANIFEST.json`, and the SHA-256 of the runner executable that
produced them. `--allow-unpinned-conformance-inputs` admits older maps only as
diagnostic evidence. A report records completeness and provenance, and
baseline comparison rejects either diagnostic form.

Manifest versions outside v8/v9 and malformed variants in supported buckets
fail closed instead of silently shrinking the denominator. Worker records
never contribute to hostable pass or subtest totals.

## Workflow

Keep generated results outside Git:

```powershell
$env:CARGO_TARGET_DIR = 'C:\t\genet-conformance-ledger-target'
$proof = 'C:\Users\mark_\Code\testing\genet\wpt-ledger\absolute-css'

cargo run -p genet-wpt --release --offline -- `
  reftest css `
  --renderer livery `
  --write-expectations "$proof\css_reftest_livery.json"

cargo run -p genet-wpt --release --offline -- `
  testharness css `
  --engine boa `
  --renderer livery `
  --write-expectations "$proof\css_testharness_livery.json"

cargo run -p genet-wpt --release --offline -- `
  conformance css `
  --engine boa `
  --renderer livery `
  --reftest-results "$proof\css_reftest_livery.json" `
  --testharness-results "$proof\css_testharness_livery.json" `
  --write-conformance "$proof\css_livery_conformance.json"
```

More than one result file may be supplied for either lane. Overlapping files
are deduplicated only when the exact status, reason, and subtest counts agree.
A conflicting duplicate or mixed runner identity within a lane rejects the
report.

To compare with a prior absolute report:

```powershell
cargo run -p genet-wpt --release --offline -- `
  conformance css `
  --engine boa `
  --renderer livery `
  --reftest-results "$proof\css_reftest_livery.json" `
  --testharness-results "$proof\css_testharness_livery.json" `
  --conformance-baseline "$proof\previous_css_livery_conformance.json"
```

The comparison reports aggregate deltas and says whether the manifest or
either lane's runner executable changed. Exact expectation comparison remains
the regression guard that exposes status churn inside the totals.

## Done condition

- Manifest tests are counted once by stable WPT URL.
- The report rejects renderer, engine, command, scope, manifest identity,
  malformed provenance, status, count, and conflicting-overlap mismatches.
- Missing hostable tests or unpinned result maps prevent baseline use.
- Worker-only variants and unsupported manifest kinds remain visible and
  cannot increase conformance totals.
- Print reftests remain unsupported until a print-media harness exists.
- The hybrid testharness route is serialized as Livery CSSOM plus Stylo
  geometry rather than mislabeled as Livery layout.
- Testharness passing subtests and observed totals are serialized
  deterministically.
- A baseline comparison names manifest growth, passing-subtest movement,
  passing-file movement, missing hostable tests, and unsupported-kind
  movement, while disclosing manifest and runner changes.
- Focused unit tests, a live manifest join, and the `genet-wpt` build pass.

## Integration boundary

Develop and review this command while K3 runs, but do not merge it into K3's
receipt stream. A runner change between K3 gates would make successive
expectation maps less directly comparable even when the renderer is
unchanged. Merge after K3s, then freeze the first complete `css` baseline.

## Instrument receipt

The first live join used the stored 2026-07-24 result maps for `css/CSS2`.
Those maps predate provenance fields, so this is an explicitly unpinned,
baseline-ineligible instrument proof:

| Measure | Absolute count |
|---|---:|
| Manifest variants | 9,254 |
| Testharness observed / hostable | 62 / 62 |
| Testharness passing subtests / observed subtests | 355 / 1,754 |
| Screen reftests observed / hostable | 6,248 / 6,248 |
| Screen reftest pass / fail / skip / error | 4,224 / 1,707 / 316 / 1 |
| Unsupported print, crash, manual, visual, or WebDriver variants | 2,944 |

The diagnostic report and its successful baseline-rejection receipt are under
`Code/testing/genet/wpt-ledger/2026-07-28_absolute_conformance_ledger`.
The first baseline must come from newly generated, pinned, complete maps after
K3s.
