# CSS animations plan

**Date:** 2026-07-09
**Status:** **A1 was already done; A2 landed 2026-07-09; A3 (events + WPT slice)
is the remaining work.** Spun out of the CSS transitions plan
(`2026-07-05_css_transitions_plan.md`), whose "Deferred (not v1)" section named
`@keyframes` as the next phase once transitions proved the tick.

*Correction, recorded 2026-07-09 before writing any code.* This plan's A1 assumed
`animation_rule` was still stubbed `None` and that `@keyframes` had to be wired
into the animation set. **Both were already true in tree**: T1 implemented
`animation_rule` against `ctx.animations` alongside `transition_rule`, `@keyframes`
parsing is Stylo's, and a probe showed a `@keyframes fade` animation already
interpolating (t=0 → 1, t=1s → 0.5). The transitions plan's own T1 landing note
says as much; the "stays `None` until the keyframes phase" line describes the
pre-T1 state. Read the code, not the plan.

What was actually broken was the **lifecycle**, described below as A2.
**Scope:** CSS animations (`@keyframes` + the `animation-*` properties) in
genet's Boa/Nova lane, style-tier, host-clocked. Same machinery as transitions:
this plan enables the sibling half that was knocked out alongside the transition
hooks, not a new subsystem.
**Related:** `2026-07-05_css_transitions_plan.md` (the machinery this reuses;
read its T1/T2/T3 first), `2026-06-24_wpt_harness_exactness_plan.md`
(measurement vehicle + the `unexpected=0` governance the WPT slice runs under),
`2026-06-24_formal_web_lessons.md` (the per-owner-batching and
rendering-update-atomicity invariants apply unchanged).

## Position

Animations live in the same style tier as transitions, for the same reason:
mid-animation `getComputedStyle` must return the interpolated value and the
animation lifecycle events must fire, so only the style system can own the
truth. netrender stays clockless (its D2 doctrine); nothing here adds state to
it. This plan reuses the transition tick spine wholesale and adds only what
`@keyframes` needs beyond a two-endpoint interpolation: named keyframe rules,
iteration, direction, fill, delay, and play-state.

## Why this is thin

The transitions v1 landing already built the shared machinery, and stylo ships
the animation half:

- The style-tier knockout was **paired**: `adapter_stylo.rs:719-886` returned
  `None` from **both** `transition_rule` and `animation_rule`, and `false` from
  both `has_css_transitions` and `has_css_animations`. T1 turned on the
  transition side; `animation_rule` was explicitly left "`None` until the
  keyframes phase" (transitions plan, T1). This plan turns on the animation
  side through the same seam.
- The persistent `DocumentAnimationSet` + animation clock on `StylePlane`, read
  by every cascade pass via `SharedStyleContext::current_time_for_animations`,
  already exists and is not transition-specific.
- The tick (`cascade::restyle_for_animation_tick`,
  `IncrementalLayout::tick_animations` / `has_active_animations`) and the host
  frame order (advance clock, drain rAF, tick, layout, paint) are built and
  under the atomic-tick invariants.
- The event-harvest pattern (`transition_events.rs`: derive lifecycle phase
  from the clock, diff against a per-session tracker, drain via
  `take_transition_events`, dispatch off the cascade through the runtime) is a
  template the animation events copy.
- Stylo's Servo-mode traversal already runs the animation start/tick algorithm
  inside `process_animations`; `@keyframes` parsing is stylo's, not genet's.

So the delta is wiring and semantics, not a new tier.

## Phases (done-conditions, not dates)

### A1: enable `animation_rule` + keyframes-to-set wiring — *already done (T1)*

`adapter_stylo.rs` already implements `animation_rule` against
`ctx.animations.get_animation_declarations(...)`, `has_css_animations` queries the
set, and Stylo parses `@keyframes` and resolves `animation-name` itself. Nothing
to do; a single-iteration animation interpolated correctly before this plan
existed. The stale doc comment ("empty until keyframes support lands") has been
corrected in place.

### A2: iteration, direction, fill, delay, play-state — ***landed 2026-07-09***

**The whole defect was one missing state transition.** Stylo creates every
`@keyframes` animation in `AnimationState::Pending` and *never promotes it*; Servo
does that from its script thread, and genet, having no such step, left every
animation `Pending` forever. That single omission produced every symptom:

- `Animation::iterate_if_necessary` returns early unless the state is `Running`,
  so the animation stayed on iteration 0. `animation-iteration-count`,
  `animation-direction`, and `infinite` all silently did nothing.
- `Animation::has_ended` returns `false` while `Pending`, so
  `get_property_declaration_at_time`'s fill-mode branch never fired. Every
  animation froze at its first iteration's end value, `fill-mode` was inert, and
  `has_active_animations` never went false, so a host would tick forever.

**Fix:** `IncrementalLayout::advance_css_animations(now)`, called from
`tick_animations` right after the clock is set. It promotes `Pending -> Running`
and calls `iterate_if_necessary` in a bounded loop, so a coarse tick that skips
whole iterations catches up. `Paused` is left alone. Three consequences:

- **An ended animation is never marked `Finished`.** The obvious completion of the
  fix (set `Finished` once `has_ended`) is a trap, and it cost a debugging round to
  find: Stylo's `process_animations_for_style` does
  `animations.retain(|a| a.state != Finished)` *during the cascade*, so storing
  `Finished` makes Stylo delete the animation in the very restyle the tick
  triggers, and `animationend` can never be harvested. This is the same hazard
  `restyle_for_animation_tick` already documents for transitions. The terminal
  phase is therefore **derived from the clock** (`Animation::has_ended(now)` is
  true for a `Running` animation past its end), which is all that Stylo's
  fill-mode branch, `has_active_animations`, and the event harvest need.
- **Animations are pruned only when canceled.** A `fill-mode: forwards` animation
  must stay in the set to keep supplying its final value; a finished
  `fill-mode: none` one is harmless to keep, because Stylo drops it from the value
  map once `has_ended`.
- **`has_active_animations` is now clock-based for animations too**, matching how
  transitions already worked: live means `Pending`/`Running` and not yet ended. An
  ended animation (even filling forwards) and a `Paused` one both report idle,
  since neither changes what is painted.

**Done when:** ~~an `alternate`, `iteration-count: 2` … animation holds its
final-frame value~~ **met.** Guards in `genet-layout`, all falsified by disabling
`advance_css_animations`:
`css_animation_interpolates_then_finishes_and_goes_idle`,
`css_animation_honors_iteration_count_and_direction` (samples at t=2.5s, a quarter
into the second iteration, where `normal` gives 0.75 and `alternate` 0.25 — a
midpoint sample cannot tell them apart), and
`css_animation_honors_fill_mode_delay_and_play_state`.

Not yet covered: fractional iteration counts, `alternate-reverse`, and
`backwards`/`both` fill. Stylo implements them; they want tests, not code.
*(Negative `animation-delay` was covered 2026-07-10 — the WPT rendering loop's
first corpus run panicked on it, exposing a stylo f32 boundary hole fixed in the
fork (`56e70cacdb`); guard
`negative_delay_and_the_f32_boundary_tick_survive`, plus the end-to-end harness
guard `animationevent_types_survives_the_rendering_session`.)*

### A3: animation events + WPT measurement

- **Animation lifecycle events — *landed 2026-07-09*.** New
  `components/genet-layout/animation_events.rs` (a peer of `transition_events.rs`,
  not a fork of it): `AnimationEventRecord` / `AnimationEventKind` and
  `harvest_animation_events`, drained by `IncrementalLayout::take_animation_events`
  in the same per-frame step as transition events. `Runtime::dispatch_animation_event`
  plus an `AnimationEvent` / `__dispatchAnimation` bootstrap pair carry them to
  script, passing the node id as a **string** (the f64-above-2^53 precision bug
  the transitions plan hit).
  - **Iteration boundaries are counted from `started_at`, not Stylo's counter.**
    `Animation::iterate` increments `KeyframesIterationState::Finite`'s counter but
    leaves `Infinite(current)` pinned at 0 forever, so the counter is unusable for
    an infinite animation. What `iterate` *always* does is advance `started_at` by
    one `duration`, and `iterate_if_necessary` refuses to advance past the final
    iteration. So the movement of `started_at`, in whole `duration`s, is exactly the
    set of non-final boundaries: finite, infinite, and coarse ticks alike.
  - **Each harvest prunes only its own kind.** The transition harvest no longer
    touches animations (it used to drop canceled ones), because the animation
    harvest must *see* a canceled animation to emit `animationcancel` before
    pruning it. The two can now be drained in either order.
  - **Phase is clock-derived**, so `animationstart` waits out `animation-delay`
    even though `advance_css_animations` promotes to `Running` immediately.
  - Guards (genet-layout): `animation_events_fire_start_then_end`,
    `animation_start_waits_for_the_delay`,
    `animation_iteration_fires_on_every_boundary_but_the_last`,
    `a_coarse_tick_emits_the_iteration_boundary_before_the_end`,
    `an_infinite_animation_emits_iterations_and_never_ends`,
    `a_finished_forwards_animation_does_not_re_emit`,
    `animation_cancel_fires_when_the_animation_is_removed`. End to end through real
    listeners on both engines (genet-scripted):
    `animation_events_dispatch_to_listeners_on_boa` / `_on_nova`.
- **Reduced motion — *landed 2026-07-09*.** `AnimationMode::Disabled` now covers
  animations: `max_transition_end` became `max_animation_end` and also considers
  finite `@keyframes` animations (current iteration's start plus the whole
  remaining active duration), so one tick lands the final frame with no
  intermediate value, and `take_animation_events` returns nothing. Guard:
  `disabled_mode_completes_animations_instantly_and_silently`.
  - **Open:** an `infinite` animation has no end to jump to, so it keeps looping
    under reduced motion. Suppressing it is a policy decision (hold the first
    frame? the fill value? cancel it?) rather than a clock jump, so it is left
    open rather than guessed at.
- **WPT slice — *baseline pinned 2026-07-09; the pass criterion is blocked*.**
  `css/css-animations` is wired into `support/wpt/check-testharness-baselines.ps1`
  under the existing `unexpected=0` governance
  (`expectations/testharness/css_animations_boa.json`, 231 tests: 8 all-pass, 105
  with-failures, 2 errored, 3 no-results, 113 skipped; **subtests 156/1198**).
  - **The animation work moved this corpus**: the same slice scored 125/1198
    subtests on the binary built immediately before A2/A3, and 156/1198 after. The
    only change between the two builds is the animation lifecycle + events work,
    so that is +31 subtests of real conformance.
  - **The event-order tests cannot pass yet, and this is not an animation gap.**
    The WPT `testharness` lane builds a `Runtime` over a `StaticDocument` and never
    constructs an `IncrementalLayout`: no animation clock, no `tick_animations`, no
    rAF pump, no `load` event. So nothing in that lane can drive an animation over
    time, whatever the style tier does. The same wall blocks the CSS **transitions**
    plan's T3 WPT slice (never wired, for this reason) and 85 of the 155 dead `dom`
    tests (see the harness-exactness plan's H6). One harness capability — a driven
    rendering loop in `genet-wpt` — unblocks all three.
    **Update 2026-07-10: that loop landed** (harness plan H7a), and two of the
    three engine levers it exposed landed with it: the `AnimationEvent` /
    `TransitionEvent` bootstrap constructors are prototype-chained (so
    `instanceof` holds; pinned end-to-end in the genet-scripted dispatch
    guards on both engines), and `computed_query` serializes the box insets
    (`left`/`right`/`top`/`bottom`) plus `transform` (an animated inset is now
    readable via `getComputedStyle`; pinned in
    `negative_delay_and_the_f32_boundary_tick_survive`). The remaining lever is
    the Web Animations API surface, still out of scope here.

**Done when:** ~~the chosen `css/css-animations` slice runs `unexpected=0` on boa
with a checked-in baseline~~ **met**, ~~and the event-order + fill/direction tests
in that slice are in the expected-pass set~~ **blocked, not on this plan**: the
WPT testharness lane has no layout session, so it cannot tick an animation. That
clause moves to whoever builds the runner's rendering loop. Everything this plan
owns — the style tier, the lifecycle, the events, reduced motion — is done and
guarded by unit + end-to-end tests on both engines.

## Deferred (not this plan)

- **Web Animations API** (`element.animate`, `getAnimations`,
  `Animation.playbackRate`). The `DocumentAnimationSet` placement does not
  preclude it; still out of scope, as in the transitions plan.
- **Scroll-driven animations** (`animation-timeline`, `scroll()` / `view()`).
  A separate timeline source; not clock-driven, so not this spine.
- **Compositor-style fast path** (paint-list property bindings so
  opacity/transform ticks skip the cascade). Same deferral as transitions: only
  if profiling shows the restyle tick hot, and it must not add a clock to
  netrender or change observable style.
- **`@property` / custom-property animation.** Follows whatever the pinned
  stylo supports; not independently pursued.

## Non-goals

- No animation runtime or wall-clock coupling in netrender (its D2 doctrine is a
  constraint on this plan).
- No genet-scripted (Servo lane) work; that lane inherits Servo's own animation
  path if it ever goes live in meerkat.
- No smolweb involvement; nematic has no CSS animation surface.
