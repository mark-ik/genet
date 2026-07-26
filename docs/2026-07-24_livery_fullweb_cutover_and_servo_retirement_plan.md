# Livery fullweb cutover and the servo-* retirement

**Date:** 2026-07-24
**Status:** in execution. D0's F3b checkpoint resolved 2026-07-25 (lane at
the revised price, multicol knocked out, see the D0 riders). Every stage
F0-F6 carries its detail, its instrument, and its receipt, and each figure
was verified against the tree on 2026-07-25 (see "Verified state" under each
stage).

Landed 2026-07-25: **F0's receipt instrument** (a consumed-set ratchet that
survives the fork's archival), **the F0 color slice** (CSS Color 4/5
including relative color syntax), and **the sub-diff instrument**, run over
CSS2, css-flexbox, and css-grid. It found that F3b's clusters are not work
items: CSS2's 449 is a long tail over 26 buckets, css-grid slices into 13
with absolutely-positioned children the clear leader, and css-flexbox does
not slice at all.

The color slice has a measured receipt, not a prediction: `css/css-color`
went from **-451 to +3091** against Stylo in one day (+3,542 subtests, zero
regressions across three runs). It was the worst directory in the F3 ledger
and is now Livery's largest lead. The specified-value layer that closed the
second half is the seam `contrast-color()` and `color-layers()` will reuse.

The css/selectors hole is closed (2026-07-26): +56 Livery over 5,376
subtests with a single 4-subtest regressing file, confirming that matching
is shared and leaving the directory out of the ledger permanently. Every F3
instrument is now complete.

Open: three unimplemented color functions (196 subtests) and the layout
clusters the sub-diff names, grid abspos first. Everything after F4 stays
gated on receipts, not dates.
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

**Cost basis revised the same day by F3b.** The lane ruling was taken partly
on the F3 testharness reading (Livery ahead everywhere, nothing structural
left). The reftest lane does not support that: Livery trails by 241 files and
**1,055 files would regress** if the default flipped today. The direction
still holds: re-seam would keep genet-layout, which *is* the Servo layout
cone the "no more servo-*" ruling exists to remove, so re-seaming buys layout
fidelity by preserving exactly what F5/F6 are meant to delete. What changed is
price: the lane owes real layout and paint work (flexbox and grid fidelity,
background paint, multicol from nothing) before F4, not just grammar slices.
Mark should see the F3b table before more is spent either way; if the answer
is that the price is too high, the fallback is the one already named (lift
the failing subsystems out of genet-layout into the lane), not a re-seam.

**RULED by Mark, 2026-07-25: lane, at the revised price.** The F3b table was
reviewed; the direction holds and the grind is accepted. Two riders:

- **Multicol is knocked out rather than built.** `column-count`,
  `column-width`, and `column-span` stay `[[unimplemented]]`, the
  css-multicol reftest directory leaves the F4 parity bar, and the
  capability returns as its own planned build after F5, a recorded
  knockout per the established practice, never a silent gap. It was the
  only structural item in either ledger; with it out, every remaining F3b
  cluster is fidelity work.
- **The genet-layout lift fallback stays in reserve.** It is invoked per
  subsystem only if the flexbox/grid fidelity pass shows taffy integration
  cannot close the 386; nothing is lifted preemptively.

## Stages, each with a receipt

- **F0 - consumed-set parity.** Close the 38 consumed longhands Livery does
  not yet implement (diffed 2026-07-24 against the audit's 126-longhand
  union; all 38 are already `[[unimplemented]]` catalog entries). These are
  ordinary H5 slices; F0 is the tracking bundle.

  **Verified state, 2026-07-25** (re-diffed the audit's overlap table against
  `components/livery/properties.toml`): 126 consumed, 88 implemented, **38
  missing, all 38 present as `[[unimplemented]]` entries**, none absent from
  the catalog. The headline holds exactly. The five lift units regroup, with
  the same total:

  | unit | count | longhands |
  |---|---:|---|
  | animation/transition controls | 7 | `animation-delay`, `animation-direction`, `animation-fill-mode`, `animation-iteration-count`, `animation-play-state`, `transition-behavior`, `transition-timing-function` |
  | background family | 6 | `background-attachment`, `background-clip`, `background-origin`, `background-position-x`, `background-position-y`, `background-size` |
  | border-image | 5 | `border-image-outset`, `border-image-repeat`, `border-image-slice`, `border-image-source`, `border-image-width` |
  | grid and alignment stragglers | 6 | `align-self`, `justify-items`, `justify-self`, `grid-auto-columns`, `grid-auto-rows`, `grid-template-areas` |
  | layout/paint/effects singles | 14 | `clear`, `clip-path`, `contain`, `content`, `direction`, `filter`, `image-rendering`, `list-style-position`, `mix-blend-mode`, `object-fit`, `perspective`, `text-overflow`, `translate`, `will-change` |

  The earlier 8/13 split for the first and last units was off by one in each
  direction; the fifth unit is layout as much as paint (`clear`, `direction`,
  and `list-style-position` are not effects), so it is renamed rather than
  re-sorted.

  **The multicol knockout does not touch F0.** `column-count`,
  `column-width`, and `column-span` are absent from the 126-longhand consumed
  set, so the 2026-07-25 knockout and F0's 126/126 receipt do not collide.
  Nothing in F0 changes because of it.

  **F0's instrument LANDED 2026-07-25.** It was missing:
  `components/livery/PROPERTY_SPACE.md` censused the whole servo-lane
  property space (implemented 95 longhands + 17 shorthands, remaining 162 +
  49), not the consumed-126 intersection, so it could not report "126/126".
  What landed:

  - `components/livery/consumed_longhands.toml`, the audit's overlap table as
    checked-in data (126 names, each tagged with the surfaces that read it).
  - `components/livery/tests/consumed_set.rs`, which asserts the intersection
    as a **ratchet** (`MAX_REMAINING`, 38 today, lower-only) and prints the
    remaining worklist with each name's catalog group on every run. A
    permanently red test would be noise and would block the workspace-green
    rule the later receipts depend on; a ratchet is green now and cannot
    silently regress. A consumed name in neither catalog table fails
    unconditionally at any ratchet value, because the census cannot count it.
  - A consumed-set section in the `import-stylo-db` census, the readable half.
  - A guard asserting the multicol knockout does not touch the consumed set,
    so the D0 ruling fails loudly if a consumer ever starts reading
    `column-*`.

  **The receipt deliberately does not live only in the generator.**
  `import-stylo-db` needs a stylo fork checkout to run at all, and the fork
  archives at F5. A receipt that needed it would die with its subject. The
  test reads two checked-in files and nothing else.

  Verified by re-running the generator against the fork at
  `b157d925267fdd37b03f43e3387ab2f0909e57b0`: **126 consumed, 88 implemented,
  38 remaining**, and the regeneration produced no catalog churn.

  Receipt: the ratchet reaches 0 and is replaced by a plain equality
  assertion; each family lands with its WPT directory delta pinned as a
  `--renderer livery` baseline.

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
  (~554), roughly 3.5-4k lines, the same order as the `calc.rs` lift H5
  already did, and harvestable under the same fork-and-own rules with
  provenance headers. Colour conversion and gamut mapping are exactly the
  "stable and spec-hardened" material the harvest plan says to lift rather
  than reinvent.

  **LANDED 2026-07-25.** `src/values/color.rs` (197 lines, a four-variant
  enum over `u8` channels) became `src/values/color/`: `mod.rs` (the `Color`
  type, serialization, interpolation), `space.rs` (fourteen color spaces and
  the conversions between them), `parse.rs` (the function grammar), `mix.rs`
  (`color-mix()` and hue interpolation). Conversion math, mixing rules, and
  the matrices are lifted from the fork's `style/color/` under the harvest
  plan's fork-and-own rules, with per-module provenance headers naming the
  rev. Two departures from the donor: no `euclid` (matrices are written in
  the spec's own row-major order and multiplied directly, so they diff
  against the spec text without transposing), and every public accessor
  resolves missing components, so NaN never escapes the module.

  Now implemented: `hsl()`/`hsla()`, `hwb()`, `lab()`, `lch()`, `oklab()`,
  `oklch()`, `color()` over all eight predefined spaces, `color-mix()` with
  all four hue-interpolation methods, **relative color syntax**, `none`
  components, slash alpha, angle units on hue, `calc()` in channel position,
  and CSS Color 4 clamping. The model carries float channels in the authored
  space, so a wide-gamut color survives until something asks for sRGB.

  **Relative color syntax** (`rgb(from red r g b)`, CSS Color 5) landed the
  same day in `relative.rs`. The origin converts into the output function's
  space and its channels bind to that function's keywords (`r g b`,
  `h s l`, `h w b`, `l a b`, `l c h`, `x y z`, plus `alpha`), which the
  channel grammar accepts directly and which are substituted into any
  `calc()` before it reaches the math program. Two details worth recording:
  the keywords are numbers in the *function's* units, so `rgb(from ...)`
  binds 0-255 while `color(from ... srgb ...)` binds 0-1 for the same color;
  and an omitted alpha inherits the origin's rather than resetting to 1. A
  `currentcolor` origin is rejected rather than silently resolving to black,
  matching `color-mix()`.

  It also exposed a general bug: a hue channel only accepted angle-typed
  math, so `hsl(calc(60 + 60) 100% 50%)` failed. A hue takes both an angle
  and a bare number of degrees; both now parse.

  **RECEIPT, measured 2026-07-25, twice.** `css/css-color` on both
  renderers; data at `Code/testing/genet/wpt-ledger/2026-07-25_color_v2/`
  (after the function grammar) and `_v4/` (after the specified-value layer):

  | | 2026-07-24 | after grammar | after specified layer |
  |---|---:|---:|---:|
  | Stylo subtests | 1107 | 1107 | 1107 (control) |
  | Livery subtests | 656 | 2678 | **4198** |
  | css-color delta | **-451** | +1571 | **+3091** |

  **+3,542 subtests in the day, zero regressions at every step**, and the
  directory the F3 ledger named as Livery's single worst (-451, the largest
  net-negative anywhere) now leads by 3,091. Stylo's total is identical
  across all three runs, so the comparison is sound rather than an
  environment artifact.

  **The specified-value layer (the second jump, +1,520).** CSSOM's
  `getPropertyValue()` returns the *specified* value, which keeps more of
  the authored shape than the computed value does: keywords stay keywords
  (`red`, `rebeccapurple`, `canvastext`), and `color-mix()` and relative
  colors serialize as themselves with only their arguments canonicalized
  (csswg-drafts #7302), resolving at computed-value time. Livery resolved
  everything at parse time. `SpecifiedColor`
  (`components/livery/src/values/color/specified.rs`) is the retained layer
  in between: validation stays the resolving parser's job (nothing it
  rejects becomes a specified value), and the capture only remembers what
  the resolver forgets. It hooks in at exactly one seam,
  `canonicalize_specified_longhand`, keyed on the property catalog's value
  type; the cascade, computed values, and paint are untouched. The same
  boundary now carries opacity's authored range (`opacity: 3` is valid,
  serializes as `3` specified, clamps computed). One regression appeared
  mid-course and was caught by the third run: the first opacity fix
  unclamped the computed level too (`opacity-computed.html` 8 to 4); moving
  the clamp back to parse and reconstructing the raw form only at the
  specified boundary restored it.

  The run also caught a real defect the unit tests missed: the legacy comma
  forms are **type-uniform**, so `rgb(10%, 20, 30%)` and
  `rgba(-2, 300, 400%, -0.5)` are invalid for mixing percentages with
  numbers even though every channel would clamp into range alone. Livery
  accepted both, before and after the slice. Fixed, with the rule extended
  to `hsl()`/`hwb()` (number-or-angle hue, both remaining channels
  percentages), and `color-invalid.html` went 8/10 to all-pass.

  **What remains in css-color: 3 files where Stylo leads, all genuinely
  unimplemented functions** (196 subtests): `color-layers()` 160, `alpha()`
  20, `contrast-color()` 16. The 813-subtest specified-value gap the first
  receipt diagnosed is closed; the two big `-valid-` files now read 997/1147
  and 523/642, with the tails being math-percentage arguments and
  `currentcolor` operands (both named gaps).

  **Unit receipts:** 32 tests in `components/livery/tests/color.rs`;
  `cargo test -p livery -p genet-livery` is 234 green, and genet-wpt,
  genet-documents, and genet-scripted build on the livery feature.

  **Three defects surfaced, all pre-existing:**

  1. `linear-gradient()` split its stops with `str::split(',')`, so any
     comma-form color inside a gradient failed to parse
     (`linear-gradient(rgb(255, 0, 0), blue)`). Fixed with a paren-aware
     splitter. It was invisible while colors serialized as hex.
  2. `rgb(300, 0, 0)` was rejected. CSS Color 4 clamps out-of-range channels
     rather than invalidating them, so it is valid and means `rgb(255, 0, 0)`.
     The old range check was wrong and a test asserted the wrong behavior.
  3. Colors serialized as `#rrggbb`. CSS Color 4 resolves the whole sRGB
     family to `rgb()`/`rgba()`, which is what `getComputedStyle` returns.
     Corrected, along with system colors now serializing lowercase
     (`canvastext`). This moved expectations in livery and genet-livery
     tests; every change was to the spec-correct value, none to accommodate
     the implementation.

  **Named gaps, not silently approximated:** `color-layers()` (160
  subtests), `contrast-color()` (16), gamut mapping (`to_srgb8` clips per
  channel rather than doing CSS Color 4's oklch chroma reduction), and
  percentage-valued `calc()` in channel position, which is rejected because
  Livery's math program reduces percentages against a length base. Gamut
  mapping is the one with teeth: clipping is visibly wrong for a saturated
  wide-gamut color, and it is a paint-quality issue rather than a parse
  failure, so it will not show up as a test error.

  The module is split under the repo's size ceiling: `mod.rs` 442,
  `parse.rs` 526, `space/mod.rs` 302, `space/rgb.rs` 251, `mix.rs` 246,
  `relative.rs` 136, `space/perceptual.rs` 113.
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

  **Verified state, 2026-07-25: the retained Stylo session is unconditional
  today, and that is the exact line F2 removes.** In
  `ports/genet-wpt/src/harness.rs`, the scripted session's constructor builds
  `IncrementalLayout::new(...)` (genet-layout, so Stylo) before it branches on
  `StyleRoute`, and only the `getComputedStyle` handler is routed: the Stylo
  arm installs `WptComputedStyle` over that layout, the Livery arm installs
  `LiveryCssom` beside it. So `--renderer livery` on the scripted lane means
  "Livery answers CSSOM, Stylo still owns geometry", which is why F2 is a
  separate stage rather than a consequence of F1. The `animating()` and
  drive-loop calls just below read the same `IncrementalLayout`, so F1's
  animation cadence hangs off that object too; F1 and F2 are cutting the same
  retained session at two seams, and F2 is the one that lets it go.
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

  **`css/selectors`: RUN 2026-07-25/26, and the falsifier reads clean.**
  The original pass could not measure it (both renderers exceeded a
  30-minute budget); run unbudgeted it costs about 2h40m per renderer, which
  is why it was the last hole. Prediction: near-parity, because the harvest
  plan keeps `selectors` a shared dependency lifted nowhere, so both lanes
  run the same matching engine and any delta is integration, not matching.

  Measured (data at `Code/testing/genet/wpt-ledger/2026-07-25_selectors/`):

  | | stylo | livery | delta |
  |---|---:|---:|---:|
  | subtests | 2145 | 2201 | **+56** |
  | files worse than the other | 1 (4 subtests) | 18 (60 subtests) | |

  A one-percent delta with **one** regressing file (`placeholder-shown.html`,
  a form-control pseudo-class, 4 subtests) confirms the shared-dependency
  reading; Livery's small lead is invalidation integration
  (`invalidation/*` accounts for most of the 60). Both renderers also took
  near-identical wall time (2h40m vs 2h43m), which is what shared-engine
  dominance looks like. **Per the falsifier rule, the directory leaves the
  ledger permanently.** Folding it in moves the F3 headline from +3,499 to
  **+3,555**, ahead in 22 of 28 measured directories. No instrument in F3
  remains open.

- **F3b - the reftest lane. RUN 2026-07-24. THE RESULT INVERTS F3.**
  Nine layout-heavy directories, both renderers, `genet-wpt reftest`.
  Reproduce with `docs/tools/ledger_reftest.sh` + `ledger_reftest_diff.py`.

  | directory | stylo | livery | delta | S-only | L-only |
  |---|---:|---:|---:|---:|---:|
  | css-flexbox | 469 | 342 | **-127** | 196 | 69 |
  | css-backgrounds | 321 | 218 | **-103** | 127 | 24 |
  | css-grid | 470 | 371 | **-99** | 190 | 91 |
  | css-position | 62 | 43 | -19 | 23 | 4 |
  | css-writing-modes | 231 | 222 | -9 | 35 | 26 |
  | css-multicol | 106 | 102 | -4 | 15 | 11 |
  | css-tables | 60 | 62 | +2 | 14 | 16 |
  | css-borders | 25 | 28 | +3 | 6 | 9 |
  | CSS2 | 4149 | 4264 | +115 | 449 | 564 |
  | **TOTAL** | **5893** | **5652** | **-241** | **1055** | **814** |

  The table is the as-run 2026-07-24 result and stays unedited. **After the
  2026-07-25 multicol knockout, the F4 bar reads over the other eight
  directories: 5,787 Stylo, 5,550 Livery, -237, and 1,040 S-only files.**
  That 1,040 is the number F4 has to drive to zero or knock out.

  **The gate reads: not ready.** On layout and paint Livery is 241 files
  behind, and the net figure understates the churn. The engines disagree on
  ~1,869 files: **1,055 that Stylo renders and Livery does not** (the real
  F4 regression count) against 814 the other way. CSS2's +115 is the clearest
  trap: a healthy-looking net that hides 449 files which would regress.
  **Net deltas are not the measure here; `S-only` is.**

  This directly contradicts the F3 reading. Testharness put Livery +3,499 and
  concluded "nothing structural remains"; that conclusion was scoped to
  parsing, computed values, and CSSOM, and it does not survive contact with
  layout. **css-flexbox is the cautionary case: +237 on testharness, -127 on
  reftest.** Parsing every flex longhand correctly is not laying flexbox out
  correctly.

  Named clusters, in gate order:
  1. **Flexbox and grid fidelity** (386 files). Taffy implements both
     algorithms, so this is integration and edge-case fidelity, not missing
     capability, and the most tractable large cluster.

     **Sub-diffed 2026-07-25**, same instrument, and the two halves are
     shaped very differently.

     **css-grid (190 S-only, 91 L-only) slices cleanly into 13 buckets:**

     | bucket | S-only | L-only | reading |
     |---|---:|---:|---|
     | abspos | 43 | 7 | **capability gap** |
     | alignment | 30 | 9 | churn |
     | grid-items | 28 | 2 | **capability gap** |
     | grid-lanes | 28 | 52 | churn |
     | subgrid | 15 | 13 | churn |
     | placement | 13 | 0 | **capability gap** |
     | grid-model | 10 | 0 | **capability gap** |
     | grid-definition | 9 | 1 | gap |
     | layout-algorithm | 9 | 0 | **capability gap** |
     | tail (4 buckets) | 5 | 7 | tail |

     By test-name theme the top item is unambiguous: `positioned-grid` (24)
     plus `orthogonal-positioned` (17) is **absolutely positioned grid
     children, about 41 files**, the single densest item in the 386. It is
     also a coherent feature rather than scattered fidelity, so it is the
     one to take first.

     **css-flexbox (196 S-only, 69 L-only) does not slice: 123 buckets, the
     largest 5.1%.** That is the finding. Flexbox is a long tail of
     individual features, so there is no big win available and no reason to
     sequence it ahead of grid. Its densest themes are
     `flex-minimum-*` (16 files, the automatic minimum size of flex items,
     a known spec corner), `table-as-*` (11, tables as flex items),
     `flex-flow` (10), and `flexbox-baseline-*` (9). Expect steady grind
     here, not a breakthrough, and take grid's abspos cluster first.
  2. **CSS2 core** (449 files). The broad fullweb body; needs its own
     sub-diff before it can be sliced, since it spans floats, inline layout,
     tables, and positioning.

     **Instrument spec.** `ledger_reftest_diff.py` already computes the
     per-directory S-only list (`regressions[d]`) and only truncates it at
     ten entries for printing, so the 449 paths exist in the data and no new
     run is needed once `target/ledger-reftest/css_CSS2_{stylo,livery}.json`
     are on disk. The sub-diff is a second reader over the same files:
     bucket each S-only path by its first segment under `css/CSS2/`, and
     bucket the 18 loose top-level files by filename stem prefix (`bidi-*`,
     `css-e-notation-*`, `inline-svg-*`). The real buckets, with corpus
     sizes for weighting (file counts under `tests/wpt/tests/css/CSS2`, not
     pass counts): tables 1239, selectors 1069, normal-flow 1045,
     margin-padding-clear 856, borders 840, positioning 721, text 716,
     backgrounds 694, generated-content 435, css1 392, floats-clear 365,
     fonts 358, syntax 319, lists 313, linebox 311, ui 242, floats 197,
     box-display 169, bidi-text 143, pagination 126, cascade 112, visufx
     104, visuren 89, visudet 61, i18n 57, and a tail of smaller ones.
     Output: bucket, S-only count, share of the 449. That table is what
     makes the cluster sliceable; until it exists, "CSS2 core" names a
     number, not a work item.

     **RUN 2026-07-25.** `docs/tools/ledger_css2_subdiff.py`, over the
     recovered data (below). The 449 spread across **26 buckets with no
     dominant one**: the largest is 12.5%. "CSS2 core" was never one work
     item.

     | bucket | S-only | share | L-only | reading |
     |---|---:|---:|---:|---|
     | normal-flow | 56 | 12.5% | 81 | churn |
     | tables | 55 | 12.2% | 3 | **capability gap** |
     | margin-padding-clear | 50 | 11.1% | 30 | churn |
     | backgrounds | 48 | 10.7% | 22 | churn |
     | positioning | 39 | 8.7% | 85 | churn |
     | borders | 35 | 7.8% | 67 | churn |
     | floats-clear | 27 | 6.0% | 43 | churn |
     | generated-content | 22 | 4.9% | 2 | **capability gap** |
     | fonts | 19 | 4.2% | 16 | churn |
     | syntax | 18 | 4.0% | 1 | **capability gap** |
     | text | 16 | 3.6% | 25 | churn |
     | floats | 15 | 3.3% | 12 | churn |
     | css1 | 9 | 2.0% | 47 | churn |
     | 13 smaller buckets | 40 | 8.9% | 130 | tail |

     A bucket where S-only is large and L-only is near zero is a missing
     capability; one where both are large is bidirectional churn, usually a
     single fidelity bug pulling in both directions. They want different
     work, which is why this table reports both and the F3b headline's net
     delta reports neither.

     **The three capability gaps are far smaller than their file counts,
     because each is dominated by one feature:**

     - **tables (55): 38 files are `fixed-table-layout-003*` variants.** One
       capability, `table-layout: fixed`, not 55 defects. With css-tables'
       own 14, table fidelity is about 69 files behind one feature.
     - **generated-content (22): 19 are `content-*`.** This is the `content`
       longhand, which is **already on F0's 38-item list**. An F0 slice buys
       these reftest files directly; F0 and F3b are not disjoint work.
     - **syntax (18): 12 are `escapes-*` and `uri-*`.** Tokenizer-level, and
       shared with the testharness lane's parsing cluster.

     The two largest churn buckets are also narrower than they look:
     normal-flow's 56 is mostly the sizing family (`max-height` 10,
     `min-width-applies-to` 5, `height` 4, `max-width` 4, plus
     replaced-element heights), which is exactly the "replaced-element
     intrinsic sizing" D0 already named as a lift candidate.
     margin-padding-clear's 50 is 36 `*-applies-to-*` files, one systematic
     pattern rather than 36 bugs.

     **The data.** Recovered 2026-07-25 and preserved at
     `Code/testing/genet/wpt-ledger/` (reftest 18 files, testharness 54).
     It reproduces the F3b table exactly: 5893 / 5652 / -241 / 1055 S-only.
     It was never in `target/`; the originating session wrote it to a
     machine-local scratchpad under `AppData/Local/Temp/claude/`, which is
     disposable. Read it with `LEDGER_OUT` pointed at that directory. No
     re-run is needed for any further CSS2 slicing.
  3. **Background paint** (127 files): sizing, positioning, repeat, and
     layering fidelity in the neutral paint path.
  4. **Multicol** (15 files, but **structurally absent**): `column-count`,
     `column-width`, and `column-span` are all `[[unimplemented]]`, and taffy
     has no multi-column algorithm. This one is build-or-knock-out, not a
     fidelity pass, and the only cluster in either ledger that is
     genuinely structural. **RULED 2026-07-25: knocked out** (see the D0
     riders); the directory leaves the F4 bar.
  5. Writing-modes (35), position (23), tables (14), borders (6): small tails.

  **F4 remains blocked**, and the reopened question is D0's cost basis, not
  its direction. See the D0 note above.
- **F4 - the default flip.**

  **Verified state, 2026-07-25.** The premise "the renderer switch exists
  only in genet-wpt's runner" holds, but the product side is further along
  and worse off than that sentence suggests:

  - The switch is `harness::StyleRoute` at
    `ports/genet-wpt/src/harness.rs:113`, selected by `ReftestRenderer` at
    `ports/genet-wpt/src/main.rs:2035`. Both are harness-local. `StyleRoute`
    appears nowhere under `components/`.
  - `genet-documents` **already carries the product-facing Livery lane**:
    `LiverySessionEngine<Fetch>` implementing `SessionEngine<Scene>`, plus
    `LiveryDocumentSession`, in `components/genet-documents/src/engines.rs`,
    behind the crate's `livery` cargo feature.
  - **No port enables that feature.** `ports/pelt/desktop` turns on
    `scripted`, `netfetch`, and `smolweb`; `livery` appears in no port
    manifest. The one enabler in the workspace is `ports/genet-wpt`, and it
    enables `genet-scripted/livery`, not `genet-documents/livery`.

  So the Livery product lane is written and unreachable: dead code in every
  shipping port. F4 is three moves, in order.

  1. **Make the lane reachable.** Enable `genet-documents/livery` in the
     ports and get it building and smoking. Until this lands, every claim
     about Livery in a product is a claim about genet-wpt only, and the
     first turn-on is where lane-versus-harness divergence will surface.
  2. **Promote compile-time to runtime.** The consumed-property audit
     already ruled that a cargo feature selects one engine per build and
     cannot supply per-document engine choice. The seam that can is
     `SessionEngine<Scene>`, which the Stylo lane and `LiverySessionEngine`
     both implement today; the profile tier picks the impl with both in the
     binary. This is a selection point, not a new abstraction.
  3. **Flip the default.** The fullweb profile routes to Livery, Stylo
     behind an explicit opt-out that F5 removes.

  **The parity bar, revised 2026-07-25.** Two changes from the original
  wording. First, the measure on the reftest lane is **`S-only`, not net
  delta**; F3b's CSS2 row (+115 net hiding 449 regressions) is why. Second,
  css-multicol leaves the bar per the D0 knockout, which drops the F3b
  numbers to **eight directories, 1,040 S-only files** (1,055 less
  multicol's 15) and a net of -237 (5,787 Stylo, 5,550 Livery). The bar:

  - Reftest lane, over the eight remaining directories: `S-only` is zero,
    or every remaining file is covered by a recorded knockout that names it.
  - Testharness lane: no directory where the Stylo pin beats the Livery pin.
  - The F0 census reads 126/126, from the generator, not by hand.
  - Every pinned baseline re-pinned under the Livery default. The runner
    already enforces this: `check_expectations` rejects a baseline whose
    recorded `renderer` differs from the run's, so stale pins fail loudly
    rather than silently passing.

  Receipt: the bar above holds, and merecat, isometry, woodshed, and hocket
  build and smoke against genet main with no renderer flag set.
- **F5 - the retirement event.** The harvest plan's trigger fires ("Livery
  takes the fullweb default with WPT parity receipts"). Five steps, each
  separately revertible, in this order:

  1. **Feature-gate genet-layout off the default build.** The first edge to
     cut is `genet-documents`, which carries `genet-layout` and
     `genet-render` as non-optional path deps
     (`components/genet-documents/Cargo.toml:37-38`); they become optional
     behind the Stylo opt-out F4 leaves in place.
  2. **Run F6a's map against the gated build**, not against today's tree.
  3. **Delete genet-layout and its consumer edges** once that map confirms
     nothing left needs them.
  4. **Archive the fork checkout** `Code/crates/stylo`; freeze the
     genet-stylo publish family at its last release.
  5. **Drop stylo_taffy and the vendored patches** that exist only for it:
     `support/patches/{stylo_taffy,taffy,ipc-channel,gpu-allocator}`.
     `support/patches/sonic-rs-0.5.8` is unrelated and stays.

  **Verified state, 2026-07-25.** No reverse build edge blocks the gate.
  `components/genet-layout/Cargo.toml` does list `genet-livery`, which would
  be an incumbent-depends-on-challenger cycle, but it is a **dev-dependency
  only**, for `components/genet-layout/tests/livery_parity.rs`. That test is
  the direct Stylo-versus-Livery comparison inside the workspace; it dies
  with genet-layout at step 3, and whatever it still asserts at that point
  should move onto a Livery-only footing first or be retired knowingly.

  Receipt: workspace green with genet-layout absent from the default build,
  then absent from the tree; no `stylo` path remains a build input.
- **F6 - the servo-* teardown** (sequenced here by Mark's ruling).

  **Pre-F5 baseline, verified 2026-07-25.** The workspace carries **48
  `servo-` prefixed packages**. That is the number F6's receipt drives to
  zero, and the denominator for everything below: 15 media, 2 orphans, 31
  survivors to rule on. Largest fan-in, with workspace aliases resolved:
  servo-malloc-size-of 27, servo-base 19, servo-url 11, servo-profile-traits
  11. These are the pre-F5 figures the next bullet exists to invalidate.

  - **F6a: recompute the dependents map after F5.** Today's fan-in counts
    are dominated by the cone F5 deletes; building equivalents for consumers
    that are about to die is waste. **Three scan traps, all live in this
    tree:**
    1. `[target.'cfg(..)'.dependencies]` sections. Missed these on
       2026-07-24 and read the gstreamer render crates as dead.
    2. `package =` renames. Missed these on 2026-07-24 and read
       servo-layout-api as dead. Live examples: `Cargo.toml:384`
       (`media = { package = "servo-media-thread" }`), plus
       `malloc_size_of`, `profile_traits`, and `deny_public_fields`, none of
       which carry the `servo-` prefix at their use sites.
    3. **Feature names colliding with alias names** (found 2026-07-25). A
       scan for the alias `profile` matches
       `support/patches/taffy/Cargo.toml:87`, which is a `[features]` entry
       `profile = ["std"]`, and reports a phantom dependent for
       servo-profile. Resolve aliases from `[workspace.dependencies]` and
       match only inside dependency tables.

    A scan that does not handle all three produces both false "dead crate"
    reads (traps 1 and 2) and false "live crate" reads (trap 3). Wrong in
    either direction costs real work here.
  - **F6b: the free deletions**, 17 crates. The 15-crate
    `components/media/` cluster is self-contained: verified 2026-07-25 that
    every reference to a `servo-media*` name outside `components/media/` is
    a declaration in the root `[workspace.dependencies]` table
    (`Cargo.toml:384`, `414-426`), never a consumer edge; pelt, genet-render,
    genet-scripted, and genet-documents carry zero media references. Plus
    the two orphans: servo-deny-public-fields (0 dependents) and
    servo-profile (0 dependents, once trap 3's phantom is discounted; its
    traits crate servo-profile-traits has 11 and is **not** an orphan).
  - **F6c: survivors get equivalents or die.** For each of the 31 still
    carrying dependents after F6a's post-F5 recount: grow the genet-native
    equivalent (genet-url, genet-pixels, a size-of trait; MIT/Apache where
    the code is clean-room, upstream names like servo-pixels are
    upstream-owned on crates.io and never reused) or delete the capability
    with a recorded knockout decision, in the same form as the multicol
    knockout. The webgl/webxr/webgpu trait family is a capability ruling for
    Mark here, not a naming exercise, and it is the largest single decision
    left in F6.
  - Receipt: zero `servo-` prefixed workspace members (48 today); genet
    workspace green; product smokes green.

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

**Ordered 2026-07-25 (audit with Mark), and where each stands:**

1. ~~**The two open instruments.**~~ **BOTH DONE.** The CSS2 sub-diff is at
   F3b cluster 2; the css/selectors run landed 2026-07-26 and reads clean
   (see F3). The testharness lane has no unmeasured directory left.
2. ~~**The F0 color slice.**~~ **DONE and measured.** `css/css-color` went
   -451 to +1571 (+2,022 subtests, zero regressions); see F0's receipt
   table. The F0 instrument landed alongside it.
3. ~~**Slice the flexbox/grid cluster.**~~ **DONE** (F3b cluster 1 carries
   both tables). The work it exposes is below.

**New work the sub-diff exposed, in rough value order:**

- **Absolutely positioned grid children** (about 41 files: css-grid's
  `abspos` bucket is 43 S-only against 7 L-only, and by theme it is
  `positioned-grid` 24 plus `orthogonal-positioned` 17). The densest single
  feature in the 386, and coherent rather than scattered.
- **`table-layout: fixed`** (about 69 files across CSS2/tables and
  css-tables, 38 of them one test family). The densest capability gap in
  CSS2, and one of the two subsystems D0 named as a lift candidate.
- **The `content` longhand.** It is on F0's 38-item list *and* it is 19 of
  CSS2/generated-content's 22 S-only files. One slice, paid twice.
- **`*-applies-to-*`** (36 files in margin-padding-clear, one systematic
  pattern) and the **replaced-element sizing family** in normal-flow (about
  26 files). The latter is D0's other named lift candidate, so measure
  before deciding whether to lift or fix in place.
- **css-flexbox** last, deliberately. Its 196 files are 123 buckets with no
  item above 5.1%, so it is grind with no leverage; every other item above
  buys more per unit of work.

**Also newly named, from the css-color receipt:**

- ~~Specified-value serialization~~ **DONE 2026-07-25** (`SpecifiedColor`,
  the +1,520 jump in the receipt; see F0).
- `color-layers()` (160), `alpha()` (20), `contrast-color()` (16): three
  unimplemented functions, in that order by value; the retained specified
  form they need now exists.

Remaining instrument debt: none. The census reports the consumed set, the
ledger is preserved outside `target/`, and the diff readers are checked in.
The sub-diff generalized while being used: `LEDGER_DIR` points it at any
directory, nested corpora bucket by subdirectory and flat ones by test-name
family.

Multicol is out per the D0 riders. The text-editing primitive has its own
founding plan (`2026-07-25_text_editing_primitive_plan.md`) and does not
ride this one. The agent-drives-pelt receipt stays queued per the direction
doc.

## Done condition

The harvest plan's retirement trigger has fired (F5), and F6c's receipt
holds: no workspace member carries the servo- prefix, and nothing outside
git history remembers the fork as a build input.
