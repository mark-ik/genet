/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Quiescence: the report a surface makes of its pending work, and the default
//! policy for "settled".
//!
//! Every automation harness that polls-and-sleeps does so because it cannot ask
//! the engine what is still in flight. The engine can answer per source, and the
//! per-source signals already exist — this module adds no mechanism, only the
//! common vocabulary the host uses to join them:
//!
//! - loads: `LoadingState` (this crate) — terminal at `Done` / `Failed`
//! - script microtasks: `script-engine-api`'s `pump` (`Quiescent` / `Pending`)
//! - timers: the runtime's `next_timer_delay()`
//! - animation-frame callbacks: the runtime's `has_animation_frame_callbacks()`
//! - declared animations: layout's `has_active_animations()`
//!
//! Only the host sees all of these at once, so the host assembles a
//! [`PendingWork`] per surface; consumers (a test harness, a WebDriver adapter,
//! an agent's observation) read it instead of sleeping.
//!
//! ## Settling vs perpetual sources
//!
//! The default [`settled`](PendingWork::settled) policy counts only work that
//! *finishes by itself*: loads, microtasks, and dirty layout. The other sources
//! are reported but do not block, each for a stated reason:
//!
//! - **Timers.** A page with `setTimeout(fn, 30_000)` is not "busy" for those
//!   thirty seconds. WebDriver's own document-readiness never waits on timers,
//!   and a harness that did would turn every long-poll page into a hang.
//! - **Animation-frame callbacks.** A one-shot rAF (schedule a measurement)
//!   settles next frame, but a rAF *loop* — every game loop, every physics
//!   surface — re-requests forever, and the two are statically
//!   indistinguishable. Blocking on rAF turns "the orrery is breathing" into
//!   "the harness never returns".
//! - **Declared animations.** CSS transitions/animations run on the engine's
//!   clock and may be infinite (`animation-iteration-count: infinite`).
//!
//! For those, the tool is a condition-wait ("element exists", "attribute
//! equals"), not a broader settle. A caller with cause to block on the
//! perpetual sources opts in explicitly via [`fully_idle`](PendingWork::fully_idle)
//! and owns the hang risk it is accepting.

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

/// One surface's pending work, by source, at the moment of asking.
///
/// A snapshot, not a subscription: settling is level-triggered (ask again), so a
/// harness loop is "apply step, ask until settled, assert" with no sleep in it.
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct PendingWork {
    /// In-flight loads (document, subresources) on this surface.
    pub loads: usize,
    /// The script engine's job queue is non-empty (`pump` would report
    /// `Pending`). `false` where the surface runs no script.
    pub microtasks: bool,
    /// Layout or paint work is scheduled but not yet performed.
    pub layout_dirty: bool,
    /// Delay to the next scheduled timer, in ms, if any timer is scheduled.
    /// Reported, never blocking (see module docs).
    pub next_timer_ms: Option<f64>,
    /// Animation-frame callbacks are requested for the next frame. Reported,
    /// never blocking: a rAF loop is indistinguishable from a one-shot.
    pub animation_frames: bool,
    /// Declared CSS transitions/animations are running. Reported, never
    /// blocking: they may be infinite.
    pub animations: bool,
}

impl PendingWork {
    /// The default settle policy: every source that finishes by itself is done.
    ///
    /// This is the harness's "the step landed" signal — loads complete, script
    /// quiescent, layout clean. It deliberately ignores timers, rAF and declared
    /// animations; see the module docs for why each would turn a live surface
    /// into a hang.
    pub fn settled(&self) -> bool {
        self.loads == 0 && !self.microtasks && !self.layout_dirty
    }

    /// Everything idle, the perpetual-capable sources included. Meaningful on a
    /// genuinely static surface; on anything animated this may simply never be
    /// true, which is the caller's risk to accept.
    pub fn fully_idle(&self) -> bool {
        self.settled()
            && self.next_timer_ms.is_none()
            && !self.animation_frames
            && !self.animations
    }
}

/// Common-minimum quiescence query, per surface. Implemented host-side (only the
/// host sees loads, script, and layout at once); read by harnesses, protocol
/// adapters, and agent observation assembly.
pub trait QuiescenceQuery {
    /// The surface's pending work right now.
    fn pending_work(&mut self) -> PendingWork;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_ignores_the_perpetual_sources() {
        // The orrery shape: nothing finishing, everything breathing.
        let breathing = PendingWork {
            next_timer_ms: Some(400.0),
            animation_frames: true,
            animations: true,
            ..PendingWork::default()
        };
        assert!(
            breathing.settled(),
            "a surface that is only animating is settled, or every game loop hangs the harness"
        );
        assert!(!breathing.fully_idle());
    }

    #[test]
    fn settled_blocks_on_work_that_finishes() {
        for pending in [
            PendingWork {
                loads: 1,
                ..PendingWork::default()
            },
            PendingWork {
                microtasks: true,
                ..PendingWork::default()
            },
            PendingWork {
                layout_dirty: true,
                ..PendingWork::default()
            },
        ] {
            assert!(!pending.settled(), "{pending:?} must block settle");
        }
        assert!(PendingWork::default().settled());
        assert!(PendingWork::default().fully_idle());
    }
}
