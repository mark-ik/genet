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
use meristem::ViewSequence;

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

/// A sized overlay box: `content` in a `position: absolute` element at `(x, y)`
/// with `width`×`height` (device px), relative to the nearest positioned ancestor.
///
/// The size-carrying companion to [`overlay_at`], for surfaces the host sizes as
/// well as places (a floating card, the comms pane, the shellbar) and would
/// otherwise re-stamp a full geometry `style` each frame. This element owns the
/// geometry `style`, so visual styling (background, border-radius, shadow,
/// `flex-direction`) belongs on the `content`'s own element, not here. See the
/// [module docs](self) for the two caller responsibilities.
pub fn overlay_rect<Seq, State, Action>(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    content: Seq,
) -> El<Seq, State, Action>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
    el::<_, State, Action>("div", content).attr(
        "style",
        format!(
            "position: absolute; left: {x}px; top: {y}px; width: {width}px; height: {height}px;"
        ),
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

/// [`anchor_point`] with **overflow-aware flip + clamp** into an available `bounds` box: if the
/// popup placed per `placement` would spill past the far edge of `bounds` on the placement axis,
/// it flips to the opposite side; the result is then clamped to keep the popup inside `bounds`.
/// The element-anchored placement a host would otherwise hand-roll (a submenu beside its parent
/// row, a card beside its node — try one side, flip on overflow, clamp on-screen), in one call.
///
/// `bounds` is `(x0, y0, x1, y1)` — the area the popup must stay inside. The popup size is
/// consulted on every side (unlike bare [`anchor_point`]), so pass a measured `popup`.
pub fn anchor_point_clamped(
    trigger: (f32, f32, f32, f32),
    popup: (f32, f32),
    placement: Placement,
    bounds: (f32, f32, f32, f32),
) -> (f32, f32) {
    let (pw, ph) = popup;
    let (bx0, by0, bx1, by1) = bounds;
    // Flip to the opposite side when the chosen side overflows that edge of `bounds`.
    let placement = match placement {
        Placement::RightOf if anchor_point(trigger, popup, Placement::RightOf).0 + pw > bx1 => {
            Placement::LeftOf
        }
        Placement::LeftOf if anchor_point(trigger, popup, Placement::LeftOf).0 < bx0 => {
            Placement::RightOf
        }
        Placement::Below if anchor_point(trigger, popup, Placement::Below).1 + ph > by1 => {
            Placement::Above
        }
        Placement::Above if anchor_point(trigger, popup, Placement::Above).1 < by0 => {
            Placement::Below
        }
        p => p,
    };
    let (x, y) = anchor_point(trigger, popup, placement);
    // A flip near the far edge can't run the popup off the near edge: clamp into `bounds`.
    let x = x.clamp(bx0, (bx1 - pw).max(bx0));
    let y = y.clamp(by0, (by1 - ph).max(by0));
    (x, y)
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
        assert_eq!(
            anchor_point(trigger, popup, Placement::Below),
            (100.0, 70.0)
        );
        // Above: same left, bottom at the trigger's top → top = 50 - popup.h.
        assert_eq!(
            anchor_point(trigger, popup, Placement::Above),
            (100.0, 20.0)
        );
        // RightOf: same top, left at the trigger's right (100 + 80).
        assert_eq!(
            anchor_point(trigger, popup, Placement::RightOf),
            (180.0, 50.0)
        );
        // LeftOf: same top, right at the trigger's left → left = 100 - popup.w.
        assert_eq!(
            anchor_point(trigger, popup, Placement::LeftOf),
            (60.0, 50.0)
        );
    }

    /// `anchor_point_clamped` keeps the chosen side when it fits, flips to the opposite side
    /// when it would overflow that edge of `bounds`, and clamps the popup inside `bounds`.
    #[test]
    fn anchor_point_clamped_flips_and_clamps() {
        let trigger = (100.0, 50.0, 80.0, 20.0); // right edge at x=180
        let popup = (40.0, 30.0);

        // Wide bounds: RightOf fits (180 + 40 = 220 <= 300), so no flip.
        assert_eq!(
            anchor_point_clamped(trigger, popup, Placement::RightOf, (0.0, 0.0, 300.0, 300.0)),
            (180.0, 50.0),
        );
        // Narrow bounds: RightOf would overflow (180 + 40 = 220 > 200) → flip to LeftOf (100 - 40 = 60).
        assert_eq!(
            anchor_point_clamped(trigger, popup, Placement::RightOf, (0.0, 0.0, 200.0, 300.0)),
            (60.0, 50.0),
        );
        // A LeftOf that runs off the left edge clamps back to x0 (flip to RightOf first, then clamp).
        let near_left = (10.0, 50.0, 20.0, 20.0); // LeftOf x = 10 - 40 = -30 < 0
        let (x, _) = anchor_point_clamped(
            near_left,
            popup,
            Placement::LeftOf,
            (0.0, 0.0, 300.0, 300.0),
        );
        assert!(
            x >= 0.0,
            "the popup is clamped on-screen, not run off the left edge"
        );
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
