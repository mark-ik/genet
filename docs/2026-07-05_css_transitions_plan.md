# CSS transitions plan

**Date:** 2026-07-05
**Status:** v1 landed 2026-07-06 (`507f1331989`: transitions + transition events + WPT slices; T1/T2 and reduced-motion marked landed inline below). CSS animations (`@keyframes`) spun to its own plan 2026-07-09 (`2026-07-09_css_animations_plan.md`), reusing this plan's tick machinery; the renderer-side fast path remains deferred here.
**Scope:** CSS transitions in genet's Boa/Nova lane, style-tier, host-clocked.
CSS animations (`@keyframes`) and any renderer-side fast path are explicitly
deferred phases, not part of the v1 done-condition.
**Related:** `archive/2026-06-24_event_loop_rigor_plan.md` (completed +
archived 2026-07-05; the atomic-tick invariants land there and are consumed
here) with residuals in `2026-07-05_event_loop_rigor_followups.md`,
`2026-06-24_wpt_harness_exactness_plan.md`
(measurement vehicle), `2026-06-24_formal_web_lessons.md` (the two event-loop
bug rules cited below).

## Position

Transitions live in the style tier: stylo's own transition machinery, advanced
by the host's frame clock, ticked through the incremental restyle path that
already runs hot in production. netrender stays clockless by its own doctrine
(`netrender/src/interpolate.rs` roadmap D2: no `Instant`, no animation runtime,
Scene snapshots replay deterministically; easing curves are pure functions the
consumer drives). Nothing in this plan adds state to netrender.

Why style-tier: mid-transition, `getComputedStyle` must return the interpolated
value, `transitionrun/start/end/cancel` must fire, and a reversing transition
must retarget from the current interpolated state. Only the style system can
own that truth; a renderer-side interpolation would lie to script.

## Current knockout (verified 2026-07-05)

The capability is knocked out in exactly two places, per the knockout strategy:

- `components/genet-layout/adapter_stylo.rs:719-886`: `animation_rule` and
  `transition_rule` return `None`; `may_have_animations`, `has_animations`,
  `has_css_animations`, `has_css_transitions` return `false`.
- `components/genet-layout/cascade.rs:751-761`: the `SharedStyleContext` gets
  `DocumentAnimationSet::default()` and `current_time_for_animations: 0.0`.

The pinned stylo already ships the machinery (`style::animation` is imported
today). Servo is the reference consumer for the call sequence; per the donor
rule, cite it for shape, don't copy wholesale.

Adjacent stubs this plan retires or touches:

- `components/script-runtime-api/lib.rs:1303`: `requestAnimationFrame` is
  `setTimeout(cb, 0)`, comment "no real frame clock yet".
- `components/genet-layout/incremental.rs`: `IncrementalLayout` already runs
  per-frame inline-style restyles (orrery motion), and
  `restyle_with_snapshots` returns a `RestyleOutcome` classifying damage as
  `RepaintOnly` (the documented hot path, fragments retained) vs re-layout.
  The transition tick reuses this spine unchanged.

## Phases

### T1: stylo transition hooks + animation set

- `IncrementalLayout` owns a real `DocumentAnimationSet` (one per document
  session; it is the document, one viewport per content card).
- Implement the adapter hooks: `transition_rule` reads the set;
  `may_have_animations` / `has_css_transitions` report from it.
  `animation_rule` stays `None` until the keyframes phase.
- After each restyle (any source: mutation batch, theme flip, inline style),
  run the start-transitions step: for elements with `transition-*` whose
  transitionable properties changed, record from/to/curve/duration in the set,
  honoring `transition-delay` and the spec's reversing-adjusted start value.
- Cascade context takes `current_time_for_animations` from the caller instead
  of 0.0.

**Done when:** a runtime test drives a `transition: opacity 100ms` style flip
through three explicit tick times (0, mid, end) and observes interpolated
`getComputedStyle` values at each, on boa and nova, with no host loop involved
(test owns the clock, matching netrender's consumer-drives-time model).

**Landed 2026-07-06.** Thinner than planned: Stylo's Servo-mode traversal
already runs the whole start/cancel algorithm (`finish_restyle` ->
`process_animations`, spec-step-commented in `style/matching.rs` /
`style/servo/animation.rs`), so T1 reduced to (a) `StylePlane` owning the
persistent `DocumentAnimationSet` + animation clock, read by every cascade
pass (`cascade.rs`'s `SharedStyleContext`), (b) honest adapter hooks
(`adapter_stylo.rs`: `transition_rule`/`animation_rule` read the set,
`has_css_transitions`/`has_css_animations` query it, `may_have_animations`
true), and (c) a tick (`cascade::restyle_for_animation_tick`,
`IncrementalLayout::tick_animations` / `has_active_animations`, T2 surface
arrived early). Two discoveries recorded for T2/T3: the
`RESTYLE_CSS_TRANSITIONS` hint requires Gecko's separate animation-only
traversal (genet does not run one), so the tick hints `RESTYLE_SELF` and rule
collection re-reads the interpolated declarations; and the Pending -> Running
-> Finished lifecycle is embedder-owned (Servo does it in its script thread),
so the tick advances states itself, which is exactly where T3's
`transitionstart`/`transitionend` will hook. Guards:
`transition_interpolates_across_animation_ticks` (genet-layout) and
`transition_interpolates_via_get_computed_style_on_boa`/`_on_nova`
(genet-scripted; a persistent session + `ComputedStyleHandler` bridge, since
`ScriptedDocument::frame` still rebuilds its session per frame, its named
retained-session follow-up).

### T2: the tick

- ~~New session entry~~ *Landed with T1 (2026-07-06):*
  `IncrementalLayout::tick_animations(dom, now_s) -> Applied` and
  `has_active_animations()`. Note: the tick hints `RESTYLE_SELF`, not
  `RESTYLE_CSS_TRANSITIONS` (animation hints assume Gecko's animation-only
  traversal; see the T1 landing note). Transform/opacity/color land on the
  `RepaintOnly` path; geometry-affecting properties take the re-layout path
  and are priced accordingly.
- Remaining: the host side. Idle surfaces must return to `dirty_tiles=0`; a
  transition that has ended must not keep a surface warm. Make this loud:
  a debug assert or diagnostic counter when a tick produces zero damage and
  zero remaining animations but the host keeps ticking.
- Two load-bearing invariants (formal-web bug rules; also in the event-loop
  rigor plan):
  - **Per-owner batching:** animation ticking is per document/surface. No
    global "animating" flag; tearing down one card neither strands nor wakes
    siblings.
  - **Rendering-update atomicity:** the host tick is one atomic per-surface
    sequence: advance clock, drain rAF callbacks, tick transitions/restyle,
    layout, paint. Never split across scheduler messages.
- rAF queue: *landed 2026-07-05* via the rigor followups
  (`Runtime::run_animation_frame_callbacks(now_ms)` +
  `has_animation_frame_callbacks()`, guards green on boa and nova; see
  `2026-07-05_event_loop_rigor_followups.md` item 3). T2's remaining work is
  ordering it inside the tick, before the transition advance, per the HTML
  update-the-rendering order.

**Done when:** a host-shaped test (pelt or the WPT runner's harness) runs a
transition to completion under a real tick loop; rAF callbacks observe
monotonically advancing interpolated values; after completion the surface
reports no active animations and produces no dirty tiles.

### T3: events + WPT measurement

- **Transition lifecycle events — *landed 2026-07-06.*** `transitionrun` /
  `transitionstart` / `transitionend` / `transitioncancel` are derived off the
  cascade and dispatched through the runtime, in order, with `propertyName` and
  `elapsedTime`. Design:
  - New `components/genet-layout/transition_events.rs`: `TransitionEventRecord`
    / `TransitionEventKind`, and `harvest_transition_events`, which diffs each
    transition's *clock-derived* lifecycle phase (not Stylo's `state`) against a
    per-session tracker and emits the ordered events. Deriving phase from the
    clock — rather than flipping Stylo state in the tick — was the key
    simplification: `PropertyAnimation::calculate_value` already clamps a
    past-end transition to its final value, and it dodges Stylo's
    `process_animations` silently dropping a just-finished transition before its
    `transitionend` is harvested (the bug the first cut hit).
  - `IncrementalLayout::take_transition_events(dom)` drains them (gated: no walk
    when idle) and prunes terminal transitions; `has_active_animations` is now
    clock-based (a transition with its end in the future), so it is the true
    idle signal independent of when the host drains. The tick's gate is
    "set non-empty" so the ending frame still lands the final value.
  - `Runtime::dispatch_transition_event` + a `TransitionEvent` /
    `__dispatchTransition` bootstrap pair. The host loop's per-frame step:
    apply/tick, `take_transition_events`, dispatch each (off the cascade).
  - Guards: `transition_events_fire_run_start_end` +
    `transition_cancel_fires_on_display_none` (genet-layout),
    `transition_events_dispatch_to_listeners_on_boa`/`_on_nova`
    (genet-scripted, end to end through real listeners).
  - *Bonus fix:* dispatching by node id exposed the pre-existing doc-tag/f64
    precision bug (`2026-07-05_event_loop_rigor_followups.md`). Passing the raw
    id as a **string** literal to the `__dispatch*` bridges (not a bare number
    that rounds above 2^53) fixed both the new path and `dispatch_event`,
    turning the previously-red `scheduler_trace_ndjson` full-suite guards green.
- **Reduced motion (host disable) — *landed 2026-07-06.*** `AnimationMode`
  (`Full` / `Disabled`) on `IncrementalLayout`, set by the host from the user's
  motion preference (`set_animation_mode`). In `Disabled`, `tick_animations`
  jumps the clock past every transition's end so the first tick lands the final
  value with no intermediate frame (monotonic: the completion clock is the
  last-created transition's `start + duration`), and `take_transition_events`
  prunes the finished transitions but returns nothing — reduced motion is
  silent. The style change still takes effect; only the animation is removed.
  Because the host frame order is apply -> tick -> paint (T2), the pre-change
  value is never painted, so the change is visually instant. Guard:
  `disabled_mode_completes_transitions_instantly_and_silently`.
  - **Author-facing `@media (prefers-reduced-motion)` — *landed 2026-07-06,
    fork-gate closed.*** Mark accepted a patched `mark-ik/stylo` (2026-07-06), so
    the feature stylo's Servo set lacked is now added there. Fork branch
    `mark-ik/servo-media-features` (based on the v0.18.0 tag, rev 8bde0e9):
    registers the `prefers-reduced-motion` atom, adds a `PrefersReducedMotion`
    value enum (shared `queries::values`), a `Device` preference field with
    getter/setter (kept out of `Device::new` so callers stay source-compatible,
    the `color_scheme` pattern), and the media-feature evaluator + descriptor.
    genet repoints all 8 stylo crate pins (workspace deps + the two
    `[patch.crates-io]` redirects) at the fork branch; `cascade.rs` gains
    `make_device_with_prefs` and `set_stylist_reduced_motion` (the live
    re-evaluation counterpart of `set_stylist_color_scheme`). Guard:
    `prefers_reduced_motion_media_query_evaluates_and_reevaluates` proves
    `@media (prefers-reduced-motion: reduce/no-preference)` selects the right
    rule at the default and flips after the setter. genet-layout suite green
    (257) against the fork. **This is the first entry in the planned Servo-mode
    media-feature parity set** (~30 standard features stylo's Servo build omits:
    hover/pointer/any-\*, prefers-contrast, forced-colors, resolution,
    orientation, display-mode, …); the fork + wiring pattern established here
    (atom + enum + Device field/setter + evaluator + genet device plumbing)
    repeats mechanically for the rest. Outstanding: consolidate the per-preference
    `Device` rebuilds into one host-set bundle (today `set_stylist_reduced_motion`
    resets color scheme), and push the remaining features when a WPT slice or
    responsive-CSS need makes them load-bearing.
- **Remaining in T3:**
  - Wire a `css/css-transitions` slice into the WPT runner under the existing
    `unexpected=0` governance; record the baseline pass set in `meta/`.
    **Blocked (established 2026-07-09):** the WPT `testharness` lane builds a
    `Runtime` over a `StaticDocument` and never constructs an `IncrementalLayout`,
    so it has no animation clock, no `tick_animations`, no rAF pump, and no `load`
    event. Nothing in that lane can drive a transition over time. The
    `css/css-animations` slice hit the same wall and was pinned as a
    status-only baseline instead (see `2026-07-09_css_animations_plan.md`, A3);
    the same capability also gates 85 of the 155 dead `dom` tests
    (`2026-06-24_wpt_harness_exactness_plan.md`, H6). One driven rendering loop in
    `genet-wpt` unblocks all three.
  - Stricter task-source queueing: events dispatch off the cascade today (post
    apply/tick), which satisfies "not synchronously inside the cascade"; routing
    them through a named task source is a rigor follow-up.

**Done when:** the chosen `css/css-transitions` slice runs `unexpected=0` on
boa, and the event-order tests in that slice (run/start/end ordering,
cancel-on-detach) are in the expected-pass set.

## Deferred (not v1)

- **CSS animations (`@keyframes`). *Promoted 2026-07-09 to
  `2026-07-09_css_animations_plan.md`.*** Same machinery, sibling hook
  (`animation_rule`, knocked out alongside `transition_rule` and left `None`
  until this phase), plus keyframe parsing and iteration/fill semantics. The
  tick is now proven, so this is an active plan, not a deferral.
- **Compositor-style fast path** (paint-list property bindings so
  opacity/transform ticks skip the cascade). Only if profiling shows the
  restyle tick hot; the orrery already sustains per-frame transform restyles,
  so the default assumption is that this is not needed. If built, it must not
  change observable style, and it must not add a clock to netrender.
- **Web Animations API** (`element.animate`, `getAnimations`). Out of scope;
  the `DocumentAnimationSet` placement should not preclude it.
- **Transition of custom properties / `@property` interpolation.** Follows
  whatever stylo's pinned release supports; not independently pursued.

## Non-goals

- No animation runtime, `Animated<T>` wrapper, or wall-clock coupling in
  netrender (its D2 doctrine is a constraint on this plan, not a gap).
- No genet-scripted (Servo lane) work; that lane inherits Servo's own
  animation path if it ever goes live in meerkat.
- No smolweb involvement; nematic has no CSS transition surface.
