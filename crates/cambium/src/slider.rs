/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A drag-to-set [`slider`], the control on top of the pointer-drag foundation.
//!
//! State is a normalized value in `0.0..=1.0`; pressing or dragging on the track
//! sets it from the pointer's position (`local.x / size.x`). Composable via
//! [`lens`](crate::lens) like the other controls; the app scales the fraction to
//! its own range.

use crate::pod::ServalElement;
use crate::{PointerEvent, ServalCtx, View, el, on_pointer};

/// The state of a [`slider`]: a normalized value in `0.0..=1.0`. Composable via
/// [`lens`](crate::lens).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Slider {
    /// The value as a fraction of the track (`0.0` = left, `1.0` = right).
    pub value: f32,
}

impl Slider {
    /// A slider at `value` (clamped to `0.0..=1.0`).
    pub fn new(value: f32) -> Self {
        Self {
            value: value.clamp(0.0, 1.0),
        }
    }
}

/// A drag-to-set slider over a [`Slider`]. The track is a `slider-track` element
/// (positioned, so the thumb anchors to it) holding an absolutely-placed
/// `slider-thumb` at `value` along it. Pressing or dragging anywhere on the
/// track sets `value` to the pointer fraction (`local.x / size.x`, clamped);
/// pointer capture keeps the drag live if the cursor leaves the track.
///
/// `+ use<>` keeps the opaque type from borrowing `state` (the percentage is
/// formatted into an owned style string).
pub fn slider(state: &Slider) -> impl View<Slider, (), ServalCtx, Element = ServalElement> + use<> {
    let pct = state.value.clamp(0.0, 1.0) * 100.0;
    // The thumb: an absolute box at `left: pct%` of the relative track.
    let thumb = el::<_, Slider, ()>("div", ())
        .attr("class", "slider-thumb")
        .attr("style", format!("position: absolute; left: {pct}%;"));
    on_pointer(
        el::<_, Slider, ()>("div", thumb)
            .attr("class", "slider-track")
            .attr("style", "position: relative;"),
        |s: &mut Slider, e: PointerEvent| {
            // Down / Move / Up all set the value from the pointer fraction, so a
            // press jumps to that point and a drag tracks it.
            if e.size.0 > 0.0 {
                s.value = (e.local.0 / e.size.0).clamp(0.0, 1.0);
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clamps() {
        assert_eq!(Slider::new(0.5).value, 0.5);
        assert_eq!(Slider::new(-1.0).value, 0.0);
        assert_eq!(Slider::new(2.0).value, 1.0);
    }
}
