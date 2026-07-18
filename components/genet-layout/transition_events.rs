/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS transition lifecycle events (`transitionrun` / `transitionstart` /
//! `transitionend` / `transitioncancel`).
//!
//! The style tier owns the truth (the `DocumentAnimationSet` on the
//! [`StylePlane`](crate::style::StylePlane)); this module turns *changes* in
//! that set into a flat list of events the host dispatches through the JS
//! runtime, off the cascade. It never dispatches: it produces
//! [`TransitionEventRecord`]s carrying the target node id, the event kind, and
//! the `propertyName` / `elapsedTime` the DOM event needs.
//!
//! Derivation is a diff against a per-session tracker of each transition's last
//! observed [`AnimationState`], so the same code covers both event sources: new
//! transitions started by a style flip (`apply`) and state advances from a
//! clock tick ([`crate::cascade::restyle_for_animation_tick`]). The spec's
//! ordering (run before start before end; cancel replaces the tail) falls out
//! of the state pairs. See <https://drafts.csswg.org/css-transitions/#events>.

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use rustc_hash::FxHashMap;
use style::animation::AnimationState;

use crate::style::StylePlane;

/// One CSS transition lifecycle event, resolved but not yet dispatched.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionEventRecord<Id> {
    /// The element the event fires at.
    pub node: Id,
    /// Which of the four transition events this is.
    pub kind: TransitionEventKind,
    /// The `propertyName` attribute: the transitioning longhand's CSS name.
    pub property_name: String,
    /// The `elapsedTime` attribute, in seconds (per the spec, the active
    /// duration already elapsed, excluding delay).
    pub elapsed_time: f64,
}

/// The four CSS transition events, in lifecycle order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionEventKind {
    /// `transitionrun`: the transition was created (before any delay).
    Run,
    /// `transitionstart`: the transition entered its active phase.
    Start,
    /// `transitionend`: the transition completed.
    End,
    /// `transitioncancel`: the transition was canceled before completing.
    Cancel,
}

impl TransitionEventKind {
    /// The DOM event type string (`"transitionrun"`, etc.).
    pub fn event_type(self) -> &'static str {
        match self {
            TransitionEventKind::Run => "transitionrun",
            TransitionEventKind::Start => "transitionstart",
            TransitionEventKind::End => "transitionend",
            TransitionEventKind::Cancel => "transitioncancel",
        }
    }
}

/// Per-session record of each transition's last observed state, keyed by
/// `(opaque node id, longhand name)`. Owned by the [`IncrementalLayout`]
/// session; [`harvest_transition_events`] diffs the live set against it.
///
/// [`IncrementalLayout`]: crate::incremental::IncrementalLayout
pub type TransitionTracker = FxHashMap<(usize, String), AnimationState>;

/// Which events a single transition's `(prior -> current)` state change emits,
/// in order. `prior == None` means the transition is newly observed.
fn events_for(
    prior: Option<&AnimationState>,
    current: &AnimationState,
) -> &'static [TransitionEventKind] {
    use AnimationState::*;
    use TransitionEventKind::*;
    match (prior, current) {
        // Newly observed.
        (None, Pending) => &[Run],
        (None, Running) => &[Run, Start],
        (None, Finished) => &[Run, Start, End],
        (None, Canceled) => &[Run, Cancel],
        // Out of the delay phase.
        (Some(Pending), Running) => &[Start],
        (Some(Pending), Finished) => &[Start, End],
        (Some(Pending), Canceled) => &[Cancel],
        // Active phase resolving.
        (Some(Running), Finished) => &[End],
        (Some(Running), Canceled) => &[Cancel],
        // No change, terminal-to-terminal, or paused (unused): nothing.
        _ => &[],
    }
}

/// The lifecycle phase a transition is in *as of the current clock*, derived
/// here rather than read from Stylo's `Transition::state`: the tick never flips
/// state (interpolation clamps a past-end transition to its final value on its
/// own), so this module owns the whole lifecycle. `Canceled` is the exception —
/// Stylo sets it during `apply` when a property stops transitioning — so it is
/// read off the transition.
fn phase_at(transition: &style::animation::Transition, now: f64) -> AnimationState {
    if transition.state == AnimationState::Canceled {
        AnimationState::Canceled
    } else if now >= transition.start_time + transition.property_animation.duration {
        AnimationState::Finished
    } else if now >= transition.start_time {
        AnimationState::Running
    } else {
        AnimationState::Pending
    }
}

/// Walk the animating elements, diff each transition's clock-derived phase
/// against `tracker`, emit the resulting events, and prune transitions that
/// have reached a terminal phase (finished/canceled) plus their tracker keys —
/// so a later re-created transition on the same property is seen fresh and
/// `has_active_animations` settles once nothing runs.
///
/// Runs *after* the tick's re-cascade (order-independent: it reads the clock,
/// not Stylo state, and the value is already correct from interpolation). `now`
/// is the current animation clock (seconds). Uses interior mutability on the
/// set, so it takes `&plane`. Cheap when nothing is animating and the tracker
/// is empty (the caller gates on that to skip the walk entirely).
pub fn harvest_transition_events<D>(
    dom: &D,
    plane: &StylePlane<D::NodeId>,
    tracker: &mut TransitionTracker,
    now: f64,
) -> Vec<TransitionEventRecord<D::NodeId>>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut events = Vec::new();

    // Map opaque node id -> live DOM node id for the animating elements, by one
    // DOM walk. Transitions on detached nodes (already gone from the tree)
    // cannot be dispatched at, so they are dropped here; the tracker prune
    // below still forgets them.
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

    // Diff every transition's clock-derived phase against the tracker.
    let mut sets = plane.animations().sets.write();
    let mut seen: std::collections::HashSet<(usize, String)> = std::collections::HashSet::new();
    for (key, set) in sets.iter() {
        let opaque = key.node.0;
        let Some(&node) = node_of.get(&opaque) else {
            continue;
        };
        for transition in &set.transitions {
            let property_name = transition
                .property_animation
                .property_id()
                .name()
                .into_owned();
            let tkey = (opaque, property_name.clone());
            seen.insert(tkey.clone());
            let phase = phase_at(transition, now);
            for &kind in events_for(tracker.get(&tkey), &phase) {
                let elapsed_time = match kind {
                    // run/start: the time already consumed by a negative delay,
                    // else 0.
                    TransitionEventKind::Run | TransitionEventKind::Start => {
                        (-transition.delay).max(0.0)
                    },
                    TransitionEventKind::End => transition.property_animation.duration,
                    TransitionEventKind::Cancel => (now - transition.start_time)
                        .clamp(0.0, transition.property_animation.duration),
                };
                events.push(TransitionEventRecord {
                    node,
                    kind,
                    property_name: property_name.clone(),
                    elapsed_time,
                });
            }
            tracker.insert(tkey, phase);
        }
    }

    // Forget transitions no longer present, and terminal ones just emitted for.
    tracker.retain(|k, phase| {
        seen.contains(k) && matches!(phase, AnimationState::Pending | AnimationState::Running)
    });

    // Drop terminal transitions from the live set so the session goes idle.
    //
    // `@keyframes` animations are **not touched here**: they are owned by
    // [`crate::animation_events::harvest_animation_events`], which must see a
    // canceled animation to emit `animationcancel` before pruning it, and which
    // deliberately keeps `Finished` ones so a `fill-mode: forwards` animation goes
    // on supplying its final value. Each harvest prunes only its own kind, so the
    // two can be drained in either order.
    for set in sets.values_mut() {
        set.transitions.retain(|t| {
            !matches!(
                phase_at(t, now),
                AnimationState::Finished | AnimationState::Canceled
            )
        });
    }
    sets.retain(|_, set| !set.is_empty());

    events
}
