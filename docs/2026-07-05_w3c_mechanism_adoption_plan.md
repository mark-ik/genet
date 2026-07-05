# W3C Mechanism Adoption Plan

**Date**: 2026-07-05
**Status**: planning. Spun out of the shell paint plan's fix-2 discussion (script cadence via
Page Visibility) after a review pass asked where else a platform mechanism should replace a host
protocol.
**Related**: [2026-07-03_shell_paint_emission_raster_plan.md](./2026-07-03_shell_paint_emission_raster_plan.md)
(focused-card churn, fix 2), the standards-correct reform precedents (document scroll as
viewport scroll; ports to components).

**Thesis**: wherever serval lacks a platform mechanism, the meerkat host invents a protocol.
Each adoption below deletes a host protocol and moves the decision into the engine where the
spec already defines its semantics. Ranked: P1 and P3 are the perf levers, P5 is the
correctness-debt lever; the rest follow as their consumers demand.

## P1. Visibility + lifecycle (fix 2, absorbs idle scheduling)

- `Document.visibilityState` + `visibilitychange` in serval-scripted; host drives it per card
  (focused card visible, preview cards hidden, closed-but-warm frozen per Page Lifecycle
  `freeze`/`resume`).
- Per spec: rAF does not fire while hidden; timers may be throttled (HTML spec license). Full
  pause = frozen.
- rAF-driven rendering + `requestIdleCallback` backfill for actor-side deferred work rides the
  same scheduler; do them together.
- Mere policy overlay, per configurability doctrine: a per-site setting
  (never-throttle / throttle / freeze), surfaced like site permissions but distinct from the
  Permissions API (which governs capabilities, not scheduling).
- **Done when** an unfocused scripted card with a timer loop runs throttled, a frozen one runs
  not at all, both resume correctly on focus, and the setting round-trips per site.

## P2. Offscreen culling: CSS containment + `content-visibility`

- The host band protocol (`request_scroll`, band caps, UV-shift composite) is a bespoke
  `content-visibility: auto`. Implement `contain` (layout/paint) and `content-visibility` in
  serval-layout so the engine skips layout+paint for offscreen subtrees itself.
- Band emission then becomes engine-native windowing; the host protocol shrinks to a viewport
  report.
- **Done when** a tall document with `content-visibility: auto` sections lays out and paints
  only the visible window plus spec-required sizing (`contain-intrinsic-size`), and the HTML
  card lane drops its band request protocol.

## P3. Theme switch as media re-evaluation (`prefers-color-scheme`)

- Today a theme switch changes the sheet set, which forces a full session rebuild (the
  persistent Stylist's sheets are fixed for its life). Express themes as media-gated rules in
  ONE fixed sheet; a switch becomes a media re-evaluation + restyle over the persistent Stylist.
- Kills the last non-resize trigger of the 34ms rebuild path.
- **Done when** toggling theme restyles without `IncrementalLayout::new` (receipts via the
  existing `rebuild_us` span) and pixel output matches the two-sheet baseline.

## P4. ResizeObserver semantics for host geometry feedback

- The shellbar-geometry churn class (host measures, then re-stamps styles, sometimes looping)
  becomes an engine-side ResizeObserver: batched delivery at the spec's lifecycle point, loop
  limit per spec (error on depth exhaustion, not oscillation).
- **Done when** the shellbar/toolbar geometry sync runs through an observer contract with no
  same-frame write-back loop, verified by the mutation-batch logs staying quiet on resize
  settle.

## P5. HTML focus model

- Sequential focus navigation (`tabindex` order), `:focus`/`:focus-visible` matching in the
  cascade, focus fixup on element removal, per WHATWG. Replaces host-owned focus bookkeeping;
  unblocks the keyboard-dispatch item ("gated on a serval focus model").
- **Done when** Tab/Shift-Tab order matches the spec on the shell document, `:focus-visible`
  styles the ring (host ring overlay retired), and removing the focused node moves focus per
  fixup rules.

## P6. Preview loading policy in lazy-load / speculation vocabulary

- Unvisited cards and kept-warm tiles are prefetch/prerender policy. Adopt the vocabulary
  (lazy, eager; prerender eagerness levels) for the settings surface even while the
  implementation stays ours. Design-language adoption first; mechanism later if a real
  `loading=lazy` consumer appears in the content lanes.
- **Done when** the card/tile warmth settings are expressed in that vocabulary in mere-domain
  settings and the plan-level flags are gone.

## P7. `will-change` as the layerization signal

- The pan escape hatch (composite the cached orrery texture at a camera offset during a
  gesture) and the retained-emission segment boundary are both compositor-layer promotion.
  Key promotion on `will-change: transform` (the camera container) so the engine chooses
  layers from the author signal, per spec, instead of bespoke partition flags.
- Feeds the deferred retained-segment-emission design (its segment container = the promoted
  layer) and the P4 raster lane's future scroll/pan fast path.
- **Done when** a `will-change: transform` container renders as a retained surface whose
  transform-only frames re-composite without re-emitting or re-rastering its subtree.

## Sequencing

P1 first (already motivated by the focused-card churn; smallest engine surface). P3 next
(deletes the remaining rebuild trigger, small). P2 and P7 are the structural pair that
eventually replaces the band protocol and the partition flags; scope them after the P1/P3
receipts. P4 and P5 when their pain next surfaces (P5 before any serious keyboard work). P6 is
naming-only and can ride any settings pass.

## Progress

- 2026-07-05: plan written from the fix-2 discussion; no code yet. P1 is the entry point.
