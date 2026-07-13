/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS animation lifecycle events (`animationstart` / `animationiteration` /
//! `animationend` / `animationcancel`).
//!
//! The sibling of [`crate::transition_events`], and the same shape: the style
//! tier owns the truth (the `DocumentAnimationSet` on the
//! [`StylePlane`](crate::style::StylePlane)); this module turns *changes* in that
//! set into a flat list of records the host dispatches through the JS runtime,
//! off the cascade. It never dispatches.
//!
//! Derivation diffs each animation against a per-session tracker of its last
//! observed phase **and iteration index**. The iteration index is what makes this
//! more than a copy of the transition harvest: `animationiteration` fires at every
//! iteration boundary except the last, and Stylo's `iterate_if_necessary` (driven
//! by [`crate::incremental::IncrementalLayout::advance_css_animations`]) refuses
//! to advance past the final iteration, so a rising `iteration_state` *is* exactly
//! the set of non-final boundaries.
//!
//! Phase is derived from the clock rather than read from `Animation::state`,
//! because the state machine promotes to `Running` as soon as the animation is
//! created, including while it is still inside its `animation-delay`.
//! `animationstart` must wait for the delay to elapse, so `started_at` (which
//! Stylo defines as creation time *plus* delay) is the boundary.
//!
//! See <https://drafts.csswg.org/css-animations/#events>.

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use rustc_hash::FxHashMap;
use style::animation::{Animation, AnimationState, KeyframesIterationState};

use crate::style::StylePlane;

/// One CSS animation lifecycle event, resolved but not yet dispatched.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationEventRecord<Id> {
    /// The element the event fires at.
    pub node: Id,
    /// Which of the four animation events this is.
    pub kind: AnimationEventKind,
    /// The `animationName` attribute: the `@keyframes` rule's name.
    pub animation_name: String,
    /// The `elapsedTime` attribute, in seconds: how long the animation has been
    /// running at this point, excluding any delay.
    pub elapsed_time: f64,
}

/// The four CSS animation events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationEventKind {
    /// `animationstart`: the animation entered its active phase (delay elapsed).
    Start,
    /// `animationiteration`: an iteration boundary that is not the last.
    Iteration,
    /// `animationend`: the final iteration completed.
    End,
    /// `animationcancel`: the animation was removed before completing.
    Cancel,
}

impl AnimationEventKind {
    /// The DOM event type string (`"animationstart"`, etc.).
    pub fn event_type(self) -> &'static str {
        match self {
            AnimationEventKind::Start => "animationstart",
            AnimationEventKind::Iteration => "animationiteration",
            AnimationEventKind::End => "animationend",
            AnimationEventKind::Cancel => "animationcancel",
        }
    }
}

/// What the tracker remembers about one animation between harvests.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackedAnimation {
    phase: AnimationState,
    /// Stylo's `Animation::started_at`, which it advances by exactly one
    /// `duration` each time it crosses an iteration boundary. Diffing it counts
    /// boundaries; see [`boundaries_crossed`].
    started_at: f64,
    /// How many iteration boundaries this animation has crossed so far, i.e. how
    /// many `animationiteration` events have already been emitted for it.
    iterations: f64,
}

/// Per-session record of each animation's last observed phase + iteration, keyed
/// by `(opaque node id, animation name)`.
pub type AnimationTracker = FxHashMap<(usize, String), TrackedAnimation>;

/// How many iteration boundaries the animation crossed since it was last seen at
/// `prior_started_at`.
///
/// Stylo's own iteration counter cannot be used: `Animation::iterate` increments
/// it only for `KeyframesIterationState::Finite`, leaving `Infinite(current)`
/// pinned at 0 forever. What `iterate` *always* does is advance `started_at` by
/// one `duration`, and `iterate_if_necessary` refuses to advance past the final
/// iteration. So the movement of `started_at`, in whole `duration`s, is exactly
/// the set of non-final boundaries crossed — for finite and infinite alike, and
/// for a coarse tick that crosses several at once.
fn boundaries_crossed(animation: &Animation, prior_started_at: f64) -> f64 {
    if animation.duration <= 0.0 {
        return 0.0;
    }
    let advanced = animation.started_at - prior_started_at;
    (advanced / animation.duration).round().max(0.0)
}

/// The animation's total active duration (excluding delay), or `None` when it
/// iterates forever.
fn active_duration(animation: &Animation) -> Option<f64> {
    match animation.iteration_state {
        KeyframesIterationState::Infinite(_) => None,
        KeyframesIterationState::Finite(_, total) => Some(total * animation.duration),
    }
}

/// The lifecycle phase as of the current clock. `Pending` means "still inside the
/// delay": `started_at` already includes `animation-delay`, so a clock before it
/// is the delay phase, whatever `Animation::state` says. `Paused` reports
/// `Running` — it holds a value and emits no events.
fn phase_at(animation: &Animation, now: f64) -> AnimationState {
    if animation.state == AnimationState::Canceled {
        return AnimationState::Canceled;
    }
    if now < animation.started_at {
        return AnimationState::Pending;
    }
    if animation.has_ended(now) {
        return AnimationState::Finished;
    }
    AnimationState::Running
}

/// Which events a single animation's `(prior -> current)` phase change emits, in
/// order. Iteration events are handled separately (they depend on the iteration
/// index, not the phase). `prior == None` means newly observed.
fn events_for(prior: Option<AnimationState>, current: AnimationState) -> &'static [AnimationEventKind] {
    use AnimationEventKind::*;
    use AnimationState::*;
    match (prior, current) {
        // Newly observed. A brand-new animation still inside its delay emits
        // nothing yet; `animationstart` waits for the active phase.
        (None, Pending) => &[],
        (None, Running) => &[Start],
        (None, Finished) => &[Start, End],
        (None, Canceled) => &[Cancel],
        // Out of the delay phase.
        (Some(Pending), Running) => &[Start],
        (Some(Pending), Finished) => &[Start, End],
        (Some(Pending), Canceled) => &[Cancel],
        // Active phase resolving.
        (Some(Running), Finished) => &[End],
        (Some(Running), Canceled) => &[Cancel],
        // A `Finished` animation lingers in the set to supply a `forwards` fill;
        // it must not re-emit. Canceled is terminal. Paused maps to Running.
        _ => &[],
    }
}

/// Walk the animating elements, diff each `@keyframes` animation's clock-derived
/// phase and iteration index against `tracker`, and emit the resulting events.
///
/// Unlike the transition harvest, this **does not prune** the live set: a
/// `Finished` animation stays to supply a `fill-mode: forwards` value, and its
/// tracker entry stays with it so it does not re-emit `animationstart` on the
/// next harvest. Tracker entries are dropped only when the animation leaves the
/// set or is canceled. Pruning of canceled animations is the transition harvest's
/// job (it holds the write lock).
///
/// `now` is the current animation clock (seconds).
pub fn harvest_animation_events<D>(
    dom: &D,
    plane: &StylePlane<D::NodeId>,
    tracker: &mut AnimationTracker,
    now: f64,
) -> Vec<AnimationEventRecord<D::NodeId>>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut events = Vec::new();

    // Map opaque node id -> live DOM node id, by one DOM walk. Animations on
    // detached nodes cannot be dispatched at, so they are skipped; the tracker
    // prune below still forgets them.
    let animating: std::collections::HashSet<usize> = {
        let sets = plane.animations().sets.read();
        sets.keys().map(|k| k.node.0).collect()
    };
    let mut node_of: FxHashMap<usize, D::NodeId> = FxHashMap::default();
    if !animating.is_empty() {
        let mut stack = vec![dom.document()];
        while let Some(node) = stack.pop() {
            for child in dom.dom_children(node) {
                stack.push(child);
            }
            let opaque = dom.opaque_id(node) as usize;
            if animating.contains(&opaque) {
                node_of.insert(opaque, node);
            }
        }
    }

    let mut sets = plane.animations().sets.write();
    let mut seen: std::collections::HashSet<(usize, String)> = std::collections::HashSet::new();
    for (key, set) in sets.iter() {
        let opaque = key.node.0;
        let Some(&node) = node_of.get(&opaque) else {
            continue;
        };
        for animation in &set.animations {
            let name = animation.name.to_string();
            let akey = (opaque, name.clone());
            seen.insert(akey.clone());

            let phase = phase_at(animation, now);
            let prior = tracker.get(&akey).cloned();
            // A newly observed animation is assumed to start where it says it
            // starts; the host drains right after `apply` creates it, so no
            // boundary has been crossed yet.
            let prior_started_at = prior
                .as_ref()
                .map(|p| p.started_at)
                .unwrap_or(animation.started_at);
            let prior_iterations = prior.as_ref().map(|p| p.iterations).unwrap_or(0.0);
            let crossed = boundaries_crossed(animation, prior_started_at);
            let iterations = prior_iterations + crossed;

            let mut push = |kind: AnimationEventKind, elapsed_time: f64| {
                events.push(AnimationEventRecord {
                    node,
                    kind,
                    animation_name: name.clone(),
                    elapsed_time,
                });
            };

            let phase_events = events_for(prior.as_ref().map(|p| p.phase.clone()), phase.clone());
            let has = |k: AnimationEventKind| phase_events.contains(&k);

            // Spec order within one harvest: `animationstart`, then every
            // iteration boundary crossed, then the terminal event. A coarse tick
            // can cross a boundary *and* the end in the same harvest (an
            // iteration-count:2 animation ticked straight from 0s to 5s), and the
            // boundary happened first in time.
            if has(AnimationEventKind::Start) {
                // A negative delay starts the animation already progressed.
                push(AnimationEventKind::Start, (-animation.delay).max(0.0));
            }

            // One `animationiteration` per boundary crossed since the last
            // harvest. Stylo never iterates past the final iteration, so every
            // boundary here is a non-final one, exactly as the spec wants. A
            // coarse tick can cross several at once.
            let mut i = prior_iterations;
            while i < iterations {
                i += 1.0;
                push(AnimationEventKind::Iteration, i * animation.duration);
            }

            if has(AnimationEventKind::End) {
                push(AnimationEventKind::End, active_duration(animation).unwrap_or(0.0));
            }
            if has(AnimationEventKind::Cancel) {
                let elapsed = iterations * animation.duration
                    + (now - animation.started_at).clamp(0.0, animation.duration);
                push(AnimationEventKind::Cancel, elapsed);
            }

            tracker.insert(
                akey,
                TrackedAnimation {
                    phase,
                    started_at: animation.started_at,
                    iterations,
                },
            );
        }
    }

    // Forget animations that left the set, and canceled ones (terminal). A
    // `Finished` entry is retained so its lingering `forwards` fill does not look
    // like a fresh animation on the next harvest.
    tracker.retain(|k, t| seen.contains(k) && t.phase != AnimationState::Canceled);

    // Drop canceled animations now that `animationcancel` has been emitted for
    // them. `Finished` ones stay: they may be filling forwards, and the clock-based
    // `has_active_animations` already reports the session idle.
    for set in sets.values_mut() {
        set.animations.retain(|a| a.state != AnimationState::Canceled);
    }
    sets.retain(|_, set| !set.is_empty());

    events
}
