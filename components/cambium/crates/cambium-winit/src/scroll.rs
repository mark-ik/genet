/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Shared scroll *policy* for Cambium winit hosts: the desktop input
//! conventions and chrome timing that every app wants identical, kept out of
//! each host's event loop. The scroll *mechanism* (routing a delta to a
//! container, thumb geometry) lives in the engine (`genet-layout`); this module
//! only decides how gestures map to axes and when overlay scrollbars show.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// Map a wheel delta to scroll axes per desktop convention: **Shift + vertical
/// wheel scrolls horizontally**. A mouse wheel only produces `dy`; holding
/// Shift turns that into sideways motion (a touchpad's two-finger horizontal
/// swipe already arrives as real `dx` and passes through untouched, as does
/// any gesture that mixes both axes).
pub fn wheel_axes(dx: f32, dy: f32, shift: bool) -> (f32, f32) {
    if shift && dx == 0.0 {
        (dy, 0.0)
    } else {
        (dx, dy)
    }
}

/// How long a scrollbar stays fully visible after its last scroll activity.
pub const SCROLLBAR_HOLD: Duration = Duration::from_millis(900);
/// How long the fade from visible to gone takes, after the hold.
pub const SCROLLBAR_FADE: Duration = Duration::from_millis(250);

/// Auto-hide clock for overlay scrollbars, keyed by scroll target (an app uses
/// the engine's `ScrollTarget`). The host notes activity when a wheel lands;
/// each target's bar is opaque for [`SCROLLBAR_HOLD`], fades over
/// [`SCROLLBAR_FADE`], then is gone — the `alpha` feeds the engine's
/// `append_scrollbars` seam. While anything is mid-hold or mid-fade the host
/// keeps requesting redraws ([`any_visible`](Self::any_visible)).
#[derive(Default)]
pub struct ScrollbarFade<K: Copy + Eq + Hash> {
    last_activity: HashMap<K, Instant>,
}

impl<K: Copy + Eq + Hash> ScrollbarFade<K> {
    pub fn new() -> Self {
        Self {
            last_activity: HashMap::new(),
        }
    }

    /// Note scroll activity on `target` at `now` (restarts its hold + fade).
    pub fn note(&mut self, target: K, now: Instant) {
        self.last_activity.insert(target, now);
    }

    /// The target's current bar opacity in `0.0..=1.0`.
    pub fn alpha(&self, target: K, now: Instant) -> f32 {
        let Some(&at) = self.last_activity.get(&target) else {
            return 0.0;
        };
        let elapsed = now.saturating_duration_since(at);
        if elapsed <= SCROLLBAR_HOLD {
            1.0
        } else {
            let fading = elapsed - SCROLLBAR_HOLD;
            (1.0 - fading.as_secs_f32() / SCROLLBAR_FADE.as_secs_f32()).max(0.0)
        }
    }

    /// Whether any bar is still visible at `now` — the host's "keep animating"
    /// signal. Fully-faded entries are dropped as a side effect, so an idle app
    /// carries no stale keys (and dead node ids age out with them).
    pub fn any_visible(&mut self, now: Instant) -> bool {
        let gone = SCROLLBAR_HOLD + SCROLLBAR_FADE;
        self.last_activity
            .retain(|_, &mut at| now.saturating_duration_since(at) < gone);
        !self.last_activity.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_turns_a_vertical_wheel_sideways() {
        assert_eq!(wheel_axes(0.0, -40.0, true), (-40.0, 0.0));
        assert_eq!(wheel_axes(0.0, -40.0, false), (0.0, -40.0));
        // A real horizontal gesture passes through even under Shift.
        assert_eq!(wheel_axes(12.0, -3.0, true), (12.0, -3.0));
    }

    #[test]
    fn bars_hold_then_fade_then_vanish() {
        let mut fade: ScrollbarFade<u8> = ScrollbarFade::new();
        let t0 = Instant::now();
        fade.note(1, t0);

        assert_eq!(fade.alpha(1, t0), 1.0, "opaque immediately");
        assert_eq!(
            fade.alpha(1, t0 + SCROLLBAR_HOLD),
            1.0,
            "still opaque at the end of the hold"
        );
        let mid = fade.alpha(1, t0 + SCROLLBAR_HOLD + SCROLLBAR_FADE / 2);
        assert!(
            (mid - 0.5).abs() < 0.05,
            "half-faded mid-fade, got {mid}"
        );
        assert_eq!(
            fade.alpha(1, t0 + SCROLLBAR_HOLD + SCROLLBAR_FADE + Duration::from_millis(1)),
            0.0,
            "gone after the fade"
        );
        // An un-noted target was never visible.
        assert_eq!(fade.alpha(2, t0), 0.0);

        assert!(fade.any_visible(t0 + SCROLLBAR_HOLD), "visible during hold");
        assert!(
            !fade.any_visible(t0 + SCROLLBAR_HOLD + SCROLLBAR_FADE + Duration::from_millis(1)),
            "nothing visible after the fade"
        );
        assert!(
            fade.last_activity.is_empty(),
            "fully-faded entries are dropped"
        );
    }
}
