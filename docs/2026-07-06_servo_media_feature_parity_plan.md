# Servo-mode media-feature parity (mark-ik/stylo)

**Date:** 2026-07-06
**Status:** complete. M0–M5 landed 2026-07-06 (the standard Servo-mode
media-feature set: the fork's `MEDIA_FEATURES` table went 7 → 33), and the
cross-cutting WPT `css/mediaqueries` reftest slice is wired under an
`unexpected=0` guard (44/93 pass on Boa, all media-feature reftests green). The
three adjacency follow-ups (matchMedia, multi-capability pointer/hover, full
forced-colors) were then taken up 2026-07-07: two landed, one hit a real Servo-mode
blocker and was reverted (see "Adjacency follow-ups" below). Fork branch
`mark-ik/servo-media-features` at `28f4c49`; genet-layout suite green (265).
**Scope:** bring stylo's Servo-mode media-feature set up to the standard
`@media` surface, in the `mark-ik/stylo` fork (branch
`mark-ik/servo-media-features`), plus the genet-side `Device` plumbing. Author
CSS media queries, evaluated correctly. Cross-project: every genet embedder
(meerkat, mere, future browsers) inherits the whole surface.
**Related:** `2026-07-05_css_transitions_plan.md` (T3 reduced-motion, the M0
slice; the WPT hook), `2026-05-20_stylo_taffy_adoption_plan.md` (the stylo pin
mechanics), memory `feedback_track_branches_not_stale_pins.md` (stylo is now an
owned fork; add missing Servo-mode capabilities there, not host workarounds).

## Why

Stylo's Servo media-feature table (`style/servo/media_features.rs`) ships ~5
real features: `width`, `resolution`, `scan`, `prefers-color-scheme`,
`device-pixel-ratio` (Gecko's table has 60). It lacks even `height`,
`orientation`, and `aspect-ratio`, so basic responsive CSS does not evaluate,
never mind accessibility (`prefers-contrast`, `forced-colors`) or interaction
(`hover`, `pointer`). This is the single biggest author-CSS-visible gap in
genet's cascade, and it is spec-conformance work every project rendering
through genet needs regardless. M0 (`prefers-reduced-motion`) proved the fork
pipeline; this plan carries it to parity.

## Current state (2026-07-06)

Servo table + the fork's M0 add = 6 upstream + `prefers-reduced-motion`. The
fork/genet wiring pattern per feature, established at M0:

1. Register the atom in `stylo_atoms/static_atoms.txt`.
2. A value enum in shared `queries::values` (matches upstream `Orientation` /
   `PrefersColorScheme`; lets container queries reuse the geometric ones).
3. A `Device` field with getter/setter, kept OUT of `Device::new` so callers
   stay source-compatible (the `color_scheme` pattern).
4. The evaluator + descriptor entry (bump the `MEDIA_FEATURES` array length).
5. genet: thread it through `cascade.rs::make_device_with_prefs`; a live
   re-evaluation setter for the host to flip it.

## Taxonomy (by embedder-input shape)

The organizing axis is what the embedder must supply, since that decides the
implementation cost and where cross-project value concentrates.

### Tier A: geometry-derived (no host input, no new state)

Read `au_viewport_size()` / `device_pixel_ratio()` already on the `Device`;
pure eval fns.

| Feature | Evaluator | Notes |
| --- | --- | --- |
| `width` | Length | Servo has it |
| `height` | Length | **missing** (basic!) |
| `aspect-ratio` | Ratio | missing |
| `orientation` | keyword (shared `Orientation`) | missing |
| `device-width` / `device-height` | Length | missing; equal to viewport (no separate screen surface) |
| `device-aspect-ratio` | Ratio | missing; deprecated but WPT-tested |
| `resolution` | Resolution | Servo has it |

### Tier B: static constants (no input, no state)

Fixed values for a bitmap screen device.

| Feature | Value | Rationale |
| --- | --- | --- |
| `color` | 8 | truecolor bits per component |
| `color-index` | 0 | not a palette device |
| `monochrome` | 0 | not monochrome |
| `grid` | 0 | bitmap, not a tty/grid device |
| `scan` | never matches | Servo has it (only `tv` has scan) |

### Tier C: user preferences (Device field + setter; host reads OS a11y)

| Feature | Enum | Notes |
| --- | --- | --- |
| `prefers-color-scheme` | `PrefersColorScheme` | Servo has it |
| `prefers-reduced-motion` | `PrefersReducedMotion` | **M0, landed** |
| `prefers-contrast` | `PrefersContrast` (no-preference/more/less/custom) | |
| `prefers-reduced-transparency` | `PrefersReducedTransparency` | |
| `inverted-colors` | `InvertedColors` (none/inverted) | query-only (see caveat) |
| `forced-colors` | `ForcedColors` (shared, exists) | query-only (see caveat) |

**Caveat (forced-colors, inverted-colors):** these also drive color
*computation* (system-color substitution, `forced-color-adjust`), not only the
query bit. This plan lands the media-query evaluation only; the full
repaint-in-system-colors behavior is a separate, larger W3C capability, tracked
elsewhere. The `Device` already stubs `forced_colors()` returning `None`;
M3 makes it read the field.

### Tier D: device capabilities (Device field + setter; host reads hardware)

| Feature | Enum | Default |
| --- | --- | --- |
| `pointer` / `any-pointer` | `Pointer` (none/coarse/fine) | fine |
| `hover` / `any-hover` | `Hover` (none/hover) | hover |
| `update` | `Update` (none/slow/fast) | fast |
| `overflow-block` / `overflow-inline` | `OverflowBlock`/`Inline` | scroll |
| `color-gamut` | `ColorGamut` (srgb/p3/rec2020) | srgb |
| `dynamic-range` / `video-dynamic-range` | `DynamicRange` (standard/high) | standard |

`pointer` and `hover` are the highest-impact of the whole plan: responsive CSS
leans on `@media (hover: hover) and (pointer: fine)`. HDR/gamut default
conservative until a host reports real display capability.

### Tier E: app/engine state (no hardware; the engine knows)

| Feature | Enum | Source |
| --- | --- | --- |
| `display-mode` | `DisplayMode` (browser/standalone/fullscreen/minimal-ui/...) | app window mode (PWA) |
| `scripting` | `Scripting` (none/initial-only/enabled) | whether genet's JS runtime is live |

## The MediaEnvironment consolidation

M0 added `prefers_reduced_motion` as its own `Device` field with a
rebuild-the-whole-Device setter, which clobbers `prefers_color_scheme` (and vice
versa). That does not scale to Tiers C/D/E. Consolidate before the bulk:

- **stylo fork:** one `MediaEnvironment` value struct (in `queries::values` or
  `device::servo`) holding every Tier C/D/E value, with a conservative
  `Default` (real desktop screen: hover, fine pointer, fast update, srgb,
  standard range, no-preference across the board, `display-mode: browser`,
  `scripting: enabled`). `ExtraDeviceData` holds one `MediaEnvironment`
  (replacing the individual `prefers_*` fields); `media_environment()` /
  `set_media_environment()` accessors, with the existing `color_scheme()` /
  `prefers_reduced_motion()` getters kept as delegating shims so genet's
  current setters keep compiling.
- **genet:** one `set_stylist_media_env(stylist, lock, viewport, quirks, env)`
  replaces the per-preference setters; `make_device_with_prefs` takes a
  `MediaEnvironment`. Tiers A/B read viewport/constants and need nothing from
  it.

This is the cross-project payoff: each embedder builds one `MediaEnvironment`
from its platform (OS accessibility + input hardware + app state) once, and gets
the whole accessibility + capability surface.

## Phases (done-conditions, not dates)

- **M0 — prefers-reduced-motion.** *Landed 2026-07-06.* Proof of the pipeline.
- **M1 — Tier A + B (geometry + constants).** *Landed 2026-07-06* (fork commit
  `bdf789c`). One fork patch: added `height`, `device-width/height`,
  `aspect-ratio`, `device-aspect-ratio`, `orientation` (reuses shared
  `Orientation`), `color`, `color-index`, `monochrome`, `grid` (the `MEDIA_FEATURES`
  table went 7 → 17) plus the 6 missing atoms. All derive from the Device
  viewport/dpr or fixed screen constants; no new Device state, no genet device
  changes. Guard: `tier_a_geometry_media_features_evaluate` proves a combined
  `@media (min-height: 500px) and (orientation: landscape) and
  (min-aspect-ratio: 5/4)` query matches a landscape 800x600 viewport and fails a
  portrait one. genet-layout suite green (258).
- **M2 — MediaEnvironment consolidation.** *Landed 2026-07-06* (fork commit
  `4c1cff3`). `MediaEnvironment` struct in shared `queries::values` (holds
  color-scheme + reduced-motion today, `Default` = light/no-preference); the
  `Device` stores one bundle (replacing the two separate fields), set atomically
  via `set_media_environment`, with the per-feature getters/setters kept as
  read-modify-write shims. genet: `make_device_with_prefs` takes a
  `MediaEnvironment`; new `set_stylist_media_env` is the atomic setter;
  `set_stylist_color_scheme` / `set_stylist_reduced_motion` read-modify-write the
  Stylist's current env (via `stylist.device().media_environment()`), so they
  preserve the other prefs. Guard: `media_environment_preferences_do_not_clobber`
  flips reduced-motion then color-scheme and asserts a `dark and reduce` combined
  rule still applies. genet-layout suite green (259). Each later phase now adds
  one `MediaEnvironment` field + evaluator.
- **M3 — Tier C (accessibility prefs).** *Landed 2026-07-06* (fork commit
  `f3ae7a1`). Added `PrefersContrast`, `PrefersReducedTransparency`,
  `InvertedColors` value enums (shared `queries::values`), gave the existing
  `ForcedColors` a `Default`, four `MediaEnvironment` fields, `Device` getters,
  and the evaluators (`MEDIA_FEATURES` 17 → 21). Guards:
  `tier_c_accessibility_media_features_evaluate` (contrast/transparency/inverted,
  each flipped in isolation) and `forced_colors_media_feature_evaluates`.
  genet-layout suite green (261).
  - **Forced-colors finding:** the query is wired, and stylo's *shared* cascade
    already color-reverts under `forced_colors().is_active()`
    (`properties/cascade.rs:310`), so genet gets partial forced-colors behavior
    for free. But the `forced-color-adjust: none` per-element opt-out is
    `#[cfg(feature = "gecko")]` (`cascade.rs:474`), so Servo mode reverts colors
    unconditionally. The full, spec-correct forced-colors behavior (honoring
    `forced-color-adjust`, system-color mapping) therefore stays deferred as a
    separate capability; only the media query is claimed here. The forced-colors
    test observes the un-reverted `none` state to stay decoupled from this.
- **M4 — Tier D (capabilities).** *Landed 2026-07-06* (fork commit `e99991a`).
  Added `Pointer`, `Hover`, `Update`, `OverflowBlock`, `OverflowInline`,
  `ColorGamut`, `DynamicRange` enums (the last two `PartialOrd` so a
  wider/higher device matches a narrower/lower query), 10 `MediaEnvironment`
  fields (pointer/any-pointer, hover/any-hover, update, overflow-block/inline,
  color-gamut, dynamic-range/video-dynamic-range), `Device` getters, and the
  evaluators (`MEDIA_FEATURES` 21 → 31). Conservative defaults: fine pointer,
  hover, fast, scroll, srgb, standard. Guard:
  `tier_d_capability_media_features_evaluate` proves pointer, hover, color-gamut
  (ordered match), and update flip from the default screen; the default screen
  matching `(hover: hover) and (pointer: fine)` holds by construction.
  genet-layout suite green (262).
  - **Simplification:** each of `pointer`/`hover` stores a single value (not
    Gecko's OR-of-capabilities bitflag), so `any-pointer` can't report both
    coarse and fine at once. Fine for a single-primary-modality device; revisit
    if a host needs multi-capability reporting.
- **M5 — Tier E (app/engine state).** *Landed 2026-07-06* (fork commit
  `c5ed58c`). Added `DisplayMode` (browser/minimal-ui/standalone/fullscreen)
  and `Scripting` (none/initial-only/enabled) enums, two `MediaEnvironment`
  fields, `Device` getters, and evaluators (`MEDIA_FEATURES` 31 → 33). Guard:
  `tier_e_state_media_features_evaluate` flips display-mode and scripting from
  the default (browser + enabled). genet-layout suite green (263).
  - **Remaining wiring:** `scripting` defaults to `enabled`; the host should set
    it from whether its script runtime is actually live (genet's static-layout
    path is `scripting: none`). That auto-wiring is a genet-host concern, not a
    stylo one; the media feature + `MediaEnvironment` field are ready for it.
- **Cross-cutting — WPT.** *Landed 2026-07-06.* Wired the `css/mediaqueries`
  **reftest** slice under the `unexpected=0` guard. Result on Boa:
  **44 passed, 4 failed, 45 skipped** (of 93). The 44 include every
  media-feature reftest the parity work targets — aspect-ratio-001..006,
  device-aspect-ratio-002/003/004/006, min-width-001, orientation, width/height
  — all rendering green through the full cascade -> layout -> paint ->
  pixel-compare lane (upstream Servo, lacking these features, would leave them
  red). The 4 failures are unrelated features: `at-custom-media-basic`
  (`@custom-media`) and `mq-calc-sign-function-003/004/005` (`calc()` `sign()` in
  MQ). The 45 skips are the `matchMedia()` testharness tests (below) plus
  non-reftests.
  - **Why reftests, not testharness:** the WPT `css/mediaqueries` *testharness*
    tests (`forced-colors.html`, `display-mode.html`, `match-media-parsing.html`,
    …) drive `window.matchMedia()`, a JS API genet does not expose — the media
    features are evaluated in the cascade, not surfaced to script. Those tests
    error on `matchMedia is not a function` and are out of scope here; a
    `matchMedia` binding over the same stylo evaluation is a separate feature.
    The pure-CSS `@media` reftests are what exercise the cascade features and run
    script-free.
  - **Harness change:** extended genet-wpt's JSON expectations mechanism
    (`--write-expectations` / `--expectations`, previously testharness-only) to
    the `reftest` command — the reftest loop now accumulates `ActualRecord`s and
    `finish_expectations` owns the exit under a checked baseline. Baseline:
    `ports/genet-wpt/expectations/reftest/css_mediaqueries_boa.json` (93 tests).
    Guard: `support/wpt/check-reftest-baselines.ps1` — **local, not CI**, because
    reftests render through the GPU (`Renderer::boot`), which the headless CI
    lacks (same posture as the fetch server-mode guard). Verified `unexpected=0`.

## Adjacency follow-ups (2026-07-07)

Three features adjacent to the media-feature set, taken up after the core plan
completed. Two landed; one is a documented Servo-mode blocker.

- **matchMedia — *landed.*** `window.matchMedia(query)` end to end, **no fork
  change needed** (it uses stylo's existing `MediaList` parse/evaluate). Engine
  side: `cascade::evaluate_media_query(stylist, query) -> (String /*serialized*/,
  bool /*matches*/)` parses a query string, serializes it (normalized; an unknown
  feature is preserved as `<general-enclosed>` and never matches), and evaluates
  against the stylist device; exposed as `IncrementalLayout::evaluate_media_query`.
  Runtime seam: a `MediaQueryHandler` trait + `set_media_query_handler` +
  `__matchMedia` native + a `matchMedia` bootstrap returning a
  `MediaQueryList {matches, media, ...}` (change events are stubs — a static
  snapshot for now). genet-scripted wires a `MediaQueryBridge` over the retained
  frame, alongside the computed-style bridge. Guards:
  `evaluate_media_query_matches_and_serializes` (genet-layout) and
  `match_media_evaluates_against_the_frame_on_boa`/`_on_nova` (genet-scripted,
  end to end).
  - **Change events (2026-07-08):** the `MediaQueryList` is now live —
    `.matches` / `.media` re-evaluate on each access, and `change` fires
    (`addEventListener('change')` / `addListener` / `onchange`) when the host
    calls the new `Runtime::notify_media_features_changed` and a query's result
    flipped (once per genuine flip, both directions). Listened MQLs are retained
    for re-evaluation (a small leak vs a real weak-ref registry). Guard:
    `match_media_change_events_on_boa`/`_on_nova` (a controllable handler flipped
    under the runtime).
  - **WPT testharness measurement (2026-07-08):** wired a `MediaQueryHandler`
    over a default 800x600 device into the genet-wpt runner
    (`genet_layout::MediaQueryEvaluator`), so the `css/mediaqueries`
    *testharness* tests (which drive `matchMedia`) now run instead of erroring.
    Result on Boa: **134/381 subtests pass** across 27 running files (2 all-pass,
    25 with-failures) — all previously 0, since `matchMedia` was undefined.
    Baseline `expectations/testharness/css_mediaqueries_boa.json`, wired into the
    CI testharness guard (`support/wpt/check-testharness-baselines.ps1` — no GPU
    needed, so unlike the reftest slice it runs in CI); `unexpected=0` verified.
- **Multi-capability pointer/hover — *landed*** (fork commit `b06d8f6`). The
  single-value M4 model couldn't express a hybrid device. Replaced the four
  single-value fields with two `PointerCapabilities` bitflag sets (primary +
  all-of), mirroring Gecko, so `any-pointer` reports both COARSE and FINE at once.
  Guard: `multi_capability_any_pointer_matches_coarse_and_fine` — a touchscreen +
  mouse device matches `(any-pointer: coarse) and (any-pointer: fine)`
  simultaneously while `(pointer: coarse)` stays false. (Supersedes M4's
  single-value simplification note.)
- **Full forced-colors color-computation — *attempted, reverted*** (fork commits
  `fe5a66e` add, `28f4c49` revert). Goal: honor `forced-color-adjust: none` in
  Servo mode (its opt-out in `properties/cascade.rs` is `#[cfg(feature = "gecko")]`).
  Enabling the `forced-color-adjust` longhand for Servo (dropping its engine gate)
  got it into the computed struct — `clone_forced_color_adjust` works and reads
  `"auto"` — but `forced-color-adjust: none` **still won't parse from CSS in Servo
  mode**, even as an inline style (stays `"auto"`, declaration silently dropped).
  So there's a *second* Servo-mode gate beyond the engine line: the property's
  parse-path registration was also never wired for Servo. Chasing it is real
  stylo-codegen spelunking, not worth blocking the other work, so it was reverted
  to avoid a broken half-feature. M3's forced-colors *media query* and Servo's
  partial (unconditional) color-reverting both still work; the per-element opt-out
  and spec-correct system-color mapping remain a separate, deferred capability.
  **Finding:** "enable the gecko property for servo" is necessary but not
  sufficient — the parse registration is a distinct gate.

## Settled design decisions

- **Enum home:** shared `queries::values` (matches upstream `Orientation` /
  `PrefersColorScheme`; enables container-query reuse of the geometric ones).
- **Defaults:** conservative real desktop screen, so an unconfigured host
  behaves like a normal screen rather than matching nothing.
- **Upstream tracking:** rebase the fork branch onto new stylo release tags (the
  `feedback_track_branches_not_stale_pins.md` discipline), moving `stylo_taffy`
  in lockstep.

## Non-goals

- The `-moz-*` Firefox-internal features (24 of Gecko's 60): skip.
- forced-colors / inverted-colors *color-computation* behavior (system-color
  substitution, forced-color-adjust): media-query only here; full behavior is a
  separate W3C capability.
- Container queries: the shared-enum placement keeps the door open, but
  `@container` size/style queries are out of scope for this plan.
