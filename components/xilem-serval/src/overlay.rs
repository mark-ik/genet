/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Host overlays / popups: an absolutely-positioned layer placed by a point.
//!
//! Roadmap item 4 of the serval-as-host track. An overlay is the simplest possible
//! thing: a `position: absolute` element placed by an inline `style` (serval gained
//! inline-style support for exactly this). serval-layout now implements full CSS 2.1
//! Appendix E stacking with z-index (`serval-layout/paint_stacking.rs`), so an
//! out-of-flow `position: absolute` box auto-lifts above in-flow content regardless
//! of document order; overlapping positioned boxes order by `(z-index, document
//! order)`. (No portal / teleport: an overlay stays a DOM child of wherever it is
//! placed — it is lifted in *paint* order, not reparented, so its position still
//! resolves against its containing block; see responsibility 1.)
//!
//! Two pieces, point-anchoring first:
//!   * [`overlay_at`] — the primitive: a positioned box at an `(x, y)` in the
//!     coordinate space of its nearest positioned ancestor.
//!   * [`anchor_point`] + [`Placement`] — the element-anchoring math: given a
//!     trigger element's laid-out box (which only the host knows, post-layout)
//!     and a [`Placement`], compute the `(x, y)` to hand to [`overlay_at`]. Pure
//!     and geometry-only (plain `f32`s, no engine types), so it stays in this
//!     headless crate; the host reads the trigger's fragment rect and calls it.
//!
//! ## Two responsibilities the caller owns
//!
//! 1. **A positioned ancestor.** `position: absolute` resolves against the
//!    nearest positioned ancestor (serval/taffy has no true viewport-fixed
//!    box). Make the app root `position: relative` so an overlay's `(x, y)` is
//!    root-relative and predictable.
//! 2. **Stacking.** A `position: absolute` overlay auto-lifts above its in-flow
//!    siblings (Appendix E out-of-flow lift), so no special ordering is needed to
//!    sit over normal content. To order two overlapping overlays, give the one that
//!    should win a higher `z-index`, or rely on document order as the tie-break at
//!    equal `z` (the later sibling wins). The old "must be last sibling" rule is
//!    obsolete.

use crate::pod::ServalElement;
use crate::{El, ServalCtx, el};
use xilem_core::ViewSequence;

/// An overlay box: `content` in a `position: absolute` element at `(x, y)`
/// (device px) relative to the nearest positioned ancestor.
///
/// The positioning rides an inline `style` attribute, so it depends on serval's
/// inline-style support. See the [module docs](self) for the two caller
/// responsibilities (a positioned ancestor, and z-index/document-order stacking).
pub fn overlay_at<Seq, State, Action>(x: f32, y: f32, content: Seq) -> El<Seq, State, Action>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
    el::<_, State, Action>("div", content).attr(
        "style",
        format!("position: absolute; left: {x}px; top: {y}px;"),
    )
}

/// Where to place a popup relative to its trigger element. Consumed by
/// [`anchor_point`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Below the trigger, left edges aligned (a dropdown).
    Below,
    /// Above the trigger, left edges aligned.
    Above,
    /// To the trigger's right, top edges aligned (a submenu).
    RightOf,
    /// To the trigger's left, top edges aligned.
    LeftOf,
}

/// The top-left `(x, y)` at which to place a popup of size `popup` relative to a
/// `trigger` box, per `placement`. All values share one coordinate space
/// (typically root-relative — the host reads the trigger's laid-out rect and the
/// result feeds [`overlay_at`]).
///
/// `trigger` is `(x, y, width, height)`; `popup` is `(width, height)`. Only
/// [`Placement::Above`] and [`Placement::LeftOf`] consult the popup size (they
/// grow back toward the trigger); [`Placement::Below`]/[`Placement::RightOf`]
/// ignore it, so a caller that hasn't measured the popup yet can pass `(0.0,
/// 0.0)` for those.
pub fn anchor_point(
    trigger: (f32, f32, f32, f32),
    popup: (f32, f32),
    placement: Placement,
) -> (f32, f32) {
    let (tx, ty, tw, th) = trigger;
    let (pw, ph) = popup;
    match placement {
        Placement::Below => (tx, ty + th),
        Placement::Above => (tx, ty - ph),
        Placement::RightOf => (tx + tw, ty),
        Placement::LeftOf => (tx - pw, ty),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `anchor_point` places the popup on the chosen side of the trigger, with
    /// the aligned edge matching and only the back-growing sides reading the
    /// popup size.
    #[test]
    fn anchor_point_per_placement() {
        // Trigger at (100, 50), 80×20. Popup 40×30.
        let trigger = (100.0, 50.0, 80.0, 20.0);
        let popup = (40.0, 30.0);

        // Below: same left, top at the trigger's bottom (50 + 20).
        assert_eq!(anchor_point(trigger, popup, Placement::Below), (100.0, 70.0));
        // Above: same left, bottom at the trigger's top → top = 50 - popup.h.
        assert_eq!(anchor_point(trigger, popup, Placement::Above), (100.0, 20.0));
        // RightOf: same top, left at the trigger's right (100 + 80).
        assert_eq!(anchor_point(trigger, popup, Placement::RightOf), (180.0, 50.0));
        // LeftOf: same top, right at the trigger's left → left = 100 - popup.w.
        assert_eq!(anchor_point(trigger, popup, Placement::LeftOf), (60.0, 50.0));
    }

    /// Below/RightOf ignore the popup size, so an unmeasured `(0, 0)` popup
    /// still anchors correctly on those sides.
    #[test]
    fn anchor_point_below_ignores_popup_size() {
        let trigger = (10.0, 10.0, 30.0, 12.0);
        assert_eq!(
            anchor_point(trigger, (0.0, 0.0), Placement::Below),
            anchor_point(trigger, (999.0, 999.0), Placement::Below),
        );
    }
}
