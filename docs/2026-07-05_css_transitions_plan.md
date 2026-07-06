# CSS transitions plan

**Date:** 2026-07-05
**Status:** plan (not started).
**Scope:** CSS transitions in serval's Boa/Nova lane, style-tier, host-clocked.
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

- `components/serval-layout/adapter_stylo.rs:719-886`: `animation_rule` and
  `transition_rule` return `None`; `may_have_animations`, `has_animations`,
  `has_css_animations`, `has_css_transitions` return `false`.
- `components/serval-layout/cascade.rs:751-761`: the `SharedStyleContext` gets
  `DocumentAnimationSet::default()` and `current_time_for_animations: 0.0`.

The pinned stylo already ships the machinery (`style::animation` is imported
today). Servo is the reference consumer for the call sequence; per the donor
rule, cite it for shape, don't copy wholesale.

Adjacent stubs this plan retires or touches:

- `components/script-runtime-api/lib.rs:1303`: `requestAnimationFrame` is
  `setTimeout(cb, 0)`, comment "no real frame clock yet".
- `components/serval-layout/incremental.rs`: `IncrementalLayout` already runs
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
traversal (serval does not run one), so the tick hints `RESTYLE_SELF` and rule
collection re-reads the interpolated declarations; and the Pending -> Running
-> Finished lifecycle is embedder-owned (Servo does it in its script thread),
so the tick advances states itself, which is exactly where T3's
`transitionstart`/`transitionend` will hook. Guards:
`transition_interpolates_across_animation_ticks` (serval-layout) and
`transition_interpolates_via_get_computed_style_on_boa`/`_on_nova`
(serval-scripted; a persistent session + `ComputedStyleHandler` bridge, since
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

- Dispatch `transitionrun`, `transitionstart`, `transitionend`,
  `transitioncancel` from animation-set state changes, as tasks on the proper
  task source (not synchronously inside the cascade).
- `prefers-reduced-motion` threaded to stylo's media evaluation as a real
  host setting, plus a host-level disable-animations setting (both surfaced as
  configuration, not hardcoded).
- Wire a `css/css-transitions` slice into the WPT runner under the existing
  `unexpected=0` governance; record the baseline pass set in `meta/`.

**Done when:** the chosen `css/css-transitions` slice runs `unexpected=0` on
boa, and the event-order tests in that slice (run/start/end ordering,
cancel-on-detach) are in the expected-pass set.

## Deferred (not v1)

- **CSS animations (`@keyframes`).** Same machinery, sibling hook
  (`animation_rule`), plus keyframe parsing and iteration/fill semantics. A
  second phase after transitions prove the tick; spin a plan then.
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
- No serval-scripted (Servo lane) work; that lane inherits Servo's own
  animation path if it ever goes live in meerkat.
- No smolweb involvement; nematic has no CSS transition surface.
