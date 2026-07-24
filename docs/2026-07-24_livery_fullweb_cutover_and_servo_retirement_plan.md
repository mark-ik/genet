# Livery fullweb cutover and the servo-* retirement

**Date:** 2026-07-24
**Status:** scoped. F0 rides the active H5 cadence; F3's ledger can start as
soon as both renderers run the same directories; everything after F4 is gated
on the D0 ruling below and on receipts, not dates.
**Decision record:** Mark, 2026-07-24: "no more servo-*. we grow our own
equivalents and obviate servo-* crates, or delete 'em," with the teardown
explicitly sequenced **after Livery replaces Stylo**. This plan defines that
cutover, the stage the
[harvest plan](./2026-07-20_stylo_harvest_into_livery_plan.md) names only as
its retirement trigger ("Livery takes the fullweb default with WPT parity
receipts"), and the teardown that rides behind it.
**Companions:** the harvest plan (H0-H6, receipts live there), the
[consumed-property audit](./2026-07-13_genet_consumed_css_property_audit.md)
(the 126-longhand bar), the
[profile ladder plan](./2026-05-12_genet_profile_ladder_plan.md).

## D0. The cutover shape: lane, not re-seam (recommendation)

Two shapes could satisfy the trigger. Re-seam keeps genet-layout's box tree
and swaps its style input from Stylo structs to Livery computed values. Lane
grows genet-livery's retained document (Livery styles, taffy geometry, its
own linebox, paint) to fullweb fidelity and flips the default to it.

**Recommendation: lane.** Evidence, verified 2026-07-24:

- genet-livery's only layout dependency is **crates.io taffy 0.12.1 with
  `float_layout`** enabled. No servo-* crate, no stylo family, no vendored
  patch. The lane is already the own-equivalent the servo-* ruling asks for;
  the `livery_taffy` idea exists today as the lane's in-crate taffy seam, and
  needs no standalone adapter crate.
- Every WPT receipt and every pinned `--renderer livery` baseline accrues on
  the lane. A re-seamed genet-layout would start its receipt history from
  zero.
- Re-seam defeats the retirement goal: genet-layout has 12 in-repo consumers
  and anchors most of the servo-* fan-in (servo-base 19 dependents,
  servo-malloc-size-of 27, servo-url 11, largely through the fullweb cone).
  Keeping its box tree keeps that cone alive.
- Both lanes are taffy-centered already; re-seam buys a second box tree that
  F5 would then delete.

**Fallback, bounded:** if the lane hits a fidelity wall on a named subsystem
(tables and replaced-element intrinsic sizing are the candidates), the answer
is lifting that subsystem from genet-layout into the lane under the harvest
plan's fork-and-own rules, never re-seaming Stylo back in.

**CONFIRMED by Mark, 2026-07-24**: "i'm ok with lane... big blast radius, big
impact. proceed." The apprehension is the right instinct and the sequencing
answers it: F3's ledger measures the radius before F4 flips anything, and
nothing is deleted until F5/F6, after the receipts exist.

## Stages, each with a receipt

- **F0 - consumed-set parity.** Close the 38 consumed longhands Livery does
  not yet implement (diffed 2026-07-24 against the audit's 126-longhand
  union; all 38 are already `[[unimplemented]]` catalog entries). Five lift
  units: animation/transition controls (8), background family (6),
  border-image (5), grid and alignment stragglers (6), paint/effects singles
  (13). These are ordinary H5 slices; F0 is the tracking bundle. Receipt:
  census reads 126/126 consumed implemented; each family lands with its WPT
  directory delta pinned as a `--renderer livery` baseline.

  **First slice, per the F3 ledger: the color subsystem.** It is the single
  biggest lever in the ledger (~1,230 subtests across css-color, CSS2
  colors-007, and the `getComputedStyle-resolved-colors` tail) and it is
  cleanly bounded. Livery today parses **only `rgb()`/`rgba()`** plus hex,
  named, `transparent`, `currentcolor`, `CanvasText`. Absent: `hsl()`,
  `hwb()`, `lab()`/`lch()`, `oklab()`/`oklch()`, `color()`, `color-mix()`,
  relative color syntax, `contrast-color()`, `color-layers()`.
  cssparser 0.37 does **not** supply this: its `color.rs` is 352 lines of
  primitives (`parse_hash_color`, `parse_named_color`,
  `PredefinedColorSpace`, alpha serialization) and the function grammar is
  the consumer's job, which is why the fork implements its own. Quarry
  sizing at the fork checkout: `style/color/` (color_function 569,
  convert 936, component 189, gamut + raytrace ~261, mix) plus
  `values/specified/color.rs` 1262 and the generics/computed/animated trio
  (~554) — roughly 3.5-4k lines, the same order as the `calc.rs` lift H5
  already did, and harvestable under the same fork-and-own rules with
  provenance headers. Colour conversion and gamut mapping are exactly the
  "stable and spec-hardened" material the harvest plan says to lift rather
  than reinvent.
- **F1 - animation and transition machinery.** The harvest plan's named H2
  follow-on: the fork's transition state machine (interrupted-transition
  reversing, per-element multi-transition maps) plus animation-* behavior,
  lifted onto the generated dispatch. This retires "animation cadence rides
  the retained Stylo session." Receipt: css-transitions and css-animations
  subsets runnable and pinned on the Livery route; where a Stylo-lane pin
  exists, Livery meets or names the gap.
- **F2 - geometry and hit testing on the lane.** Livery fragments answer
  elementFromPoint and pointer targeting; scroll and client geometry
  (cssom-view's read surface) come from the lane's layout; genet-scripted
  drops its retained Stylo session for geometry. This is the moment scripted
  WPT runs pure Livery. Receipt: the cssom-view and hit-testing subsets
  pinned on Livery; the Stylo session in genet-scripted is behind an
  opt-back-in flag with nothing on by default requiring it.
- **F3 - the fullweb fidelity ledger. First pass RUN 2026-07-24 (testharness
  lane).** Both renderers over the same 27 `css/` directories, Boa,
  `--write-expectations` diffed per file. Reproduce with
  `docs/tools/ledger_run.sh` + `ledger_diff.py`.

  **Headline: Livery leads the testharness lane by +3,499 subtests**
  (11,492 vs 7,993), ahead in 21 of 27 directories, and runs about 2.4x
  faster on the same corpus. The layout directories predicted to be Stylo
  strongholds are Livery wins: sizing +471, grid +334, align +280, position
  +253, flexbox +237, writing-modes +48, CSS2 +55.

  **The caveat that bounds this result, stated first because it is the
  honest scope limit:** this is the *testharness* lane only. It measures
  parsing, computed values, and CSSOM. It does **not** measure layout and
  paint fidelity, which live in reftests (`genet-wpt reftest`, needs a GPU
  and was not run here). The directories that scored identically under both
  renderers are the tell: css-multicol skips 617 of 708 files, css-tables
  195 of 328, css-borders and cssom-view likewise flat. Identical scores
  there mean *not measured*, not parity. **A reftest pass over the same
  directories is required before F4 can claim parity**, and it is the one
  place where the "Livery is ahead" reading could still invert.

  Net-negative directories (the real slices): css-color -451, css-images
  -207, css-pseudo -35, cssom -34, css-cascade -5.

  Named clusters, grouped by cause rather than by directory:
  1. **Modern color syntax** (~1,230 subtests, the largest by far):
     relative color 583->12, `color-mix()` 230->1, `color-layers()` 160->0,
     `color()` 81->0, `contrast-color()` 16->0, alpha parsing 20->0, plus
     most of CSS2 `syntax/colors-007.html` 288->141. Livery's hex and named
     parsing are already complete and correct (it uses the same shared
     `cssparser` entry points Stylo does); the gap is entirely CSS Color
     4/5 function grammar.
  2. **Gradients** (~600): `gradient-interpolation-method` 585->0,
     gradient-position 14->0, conic-gradient calc angles.
  3. **CSS Values 5 advanced** (~220): `attr()` typed forms,
     `if-conditionals` 19->0, `random()`, minmax angle serialization.
  4. **Property-grammar breadth**, the recurring `parsing/*-valid.html`
     family across align, backgrounds, box, text, position, transitions,
     transforms, fonts, flexbox, sizing (~200 total, each file small).
     This is the same surface F0 addresses; expect F0 to close much of it.
  5. **Grid template grammar** (~160): subgrid 38->0, `repeat()` intrinsic
     21->0 (x2), template serialization.
  6. **Variables in animations** (~68) and **CSSOM serialization breadth**
     (~52, mostly `serialize-values.html` 529->497).
  7. **Pseudo-elements** (~35): replaced-element pseudos, highlight cascade.
  8. **Cascade `revert`/`revert-layer`** (~7), small and self-contained.

  Every cluster is grammar, serialization, or function coverage. None is
  structural: nothing in this pass says the lane's architecture cannot host
  fullweb. That is the finding that makes the lane ruling safe on the
  measured surface, and exactly what the reftest pass must confirm on the
  unmeasured one.

  Done when no directory remains where the Stylo lane's pinned baseline
  beats the Livery lane's, **on both the testharness and reftest lanes**.

  **`css/selectors` is unmeasured** in this pass: both renderers exceeded a
  30-minute per-run budget before writing expectations (it is the largest
  directory, `:has()` invalidation included). Its information value is low
  by construction — the harvest plan keeps `selectors` a shared dependency
  lifted nowhere, so both lanes run the same matching engine and any delta
  is integration, not matching. Re-running it with no timeout; fold the
  result in when it lands.

- **F3b - the reftest lane (the gate F3 does not close).** Same directory
  list, `genet-wpt reftest <dir> --renderer {stylo,livery}
  --write-expectations`, which is already wired for both renderers
  (`render_html_livery`) and needs a GPU. This is where layout and paint
  fidelity actually live, and where the F3 result could invert: only two
  reftest baselines exist today (css_mediaqueries, css_position), both
  Stylo-era. Priority directories are exactly the ones testharness could
  not see: CSS2, css-tables, css-multicol, css-flexbox, css-grid,
  css-position, css-backgrounds, css-borders, css-writing-modes. Receipt:
  a reftest ledger table in the same shape as F3's, and every Livery-losing
  cluster named. **F4 may not claim parity until this exists.**
- **F4 - the default flip.** Today the renderer switch exists only in
  genet-wpt's runner. Add the product-facing selection at the profile tier
  (genet-host-api / genet-documents): the fullweb profile routes to Livery by
  default, Stylo behind an explicit opt-out that F5 removes. Receipt: every
  pinned baseline re-pinned under the Livery default; merecat, isometry,
  woodshed, and hocket build and smoke against genet main with no renderer
  flag set.
- **F5 - the retirement event.** The harvest plan's trigger fires:
  genet-layout and its consumer edges come off the default build
  (feature-gated first, deleted when F6a's map confirms nothing left needs
  them); the fork checkout `Code/crates/stylo` archives; the genet-stylo
  publish family freezes at its last release; stylo_taffy and the vendored
  taffy/ipc-channel/gpu-allocator patches drop with their consumer.
- **F6 - the servo-* teardown** (sequenced here by Mark's ruling).
  - **F6a: recompute the dependents map after F5.** Today's fan-in counts
    are dominated by the cone F5 deletes; building equivalents for consumers
    that are about to die is waste. The scan must parse
    `[target.'cfg(..)'.dependencies]` sections and resolve `package =`
    renames; both traps produced false "dead crate" reads on 2026-07-24
    (the gstreamer render crates and servo-layout-api respectively).
  - **F6b: the free deletions.** The 15-crate `components/media/` cluster is
    self-contained (verified: zero consumers outside itself; pelt,
    genet-render, genet-scripted, and genet-documents carry zero media
    references), plus the orphans servo-deny-public-fields and servo-profile
    (the traits crate has 11 dependents; the implementation has none).
  - **F6c: survivors get equivalents or die.** For each servo-* crate still
    carrying dependents after F6a: grow the genet-native equivalent
    (genet-url, genet-pixels, a size-of trait; MIT/Apache where the code is
    clean-room, upstream names like servo-pixels are upstream-owned on
    crates.io and never reused) or delete the capability with a recorded
    knockout decision. The webgl/webxr/webgpu trait family is a capability
    ruling for Mark here, not a naming exercise.
  - Receipt: zero `servo-` prefixed workspace members; genet workspace green;
    product smokes green.

## Non-goals, named

- No re-seam of Stylo into anything after F5; the fallback path lifts
  genet-layout subsystems, not Stylo.
- No new fork divergence beyond keeping the incumbent green until F5.
- No upstream Servo PRs.
- No media-stack replacement at F6b; if a product later wants engine-side
  audio/video it arrives as its own planned capability, not a resurrection.
- No date estimates. The ledger (F3) is the only honest scope instrument for
  the long pole, and receipts gate every flip.

## Sequencing

F0 through F2 ride the current H5 cadence and the live lane session. F3 can
start now in parallel (it is read-only over both renderers). F4 needs D0
confirmed plus F0-F3 receipts. F5 fires the harvest plan's trigger. F6 is
strictly after F5, per the 2026-07-24 ruling, with F6b's media knockout held
to that sequencing even though it is technically independent today.

## Done condition

The harvest plan's retirement trigger has fired (F5), and F6c's receipt
holds: no workspace member carries the servo- prefix, and nothing outside
git history remembers the fork as a build input.
