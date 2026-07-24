/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A split: two slots and a draggable divider — the pane furniture as a
//! component.
//!
//! Consumer-pull (merecat, 2026-07-17): the frisket pane tiling hand-computes
//! its rects and has no divider gesture; the Workbench's platen tiling is about
//! to want the same furniture. The split owns the seam: the divider element,
//! its drag and keyboard resize, and the geometry both sides sit in.
//!
//! The ratio is the caller's state (like the grid's sort and the strip's
//! selection): the split renders whatever the caller says and reports the
//! change. The geometry is pure math on that state — [`Split::slots`] /
//! [`Split::divider_rect`] — and the view is built FROM those numbers, so a
//! tiling host that places its own surfaces can call the same functions and
//! cannot drift from what the component drew. (That is the deliberate inverse
//! of the tab strip, whose flex-and-text geometry is knowable only from the
//! layout: here the host needs the rects *before* layout, every frame, so the
//! geometry is state math and the DOM follows it.)
//!
//! The divider is an ARIA separator: `role="separator"`, `aria-orientation`,
//! `aria-valuenow`, focusable, arrow keys resize (Home/End to the clamps), and
//! a pointer drag rides [`on_pointer`]'s capture. The host styles
//! `split-divider` (and `split`, `split-slot`); geometry is inline.

use crate::pod::GenetElement;
use crate::{
    GenetCtx, Key, NamedKey, PointerEvent, PointerPhase, View, el, focusable, on_key, on_pointer,
};

/// Which way a split divides its area. Named for the axis the divider moves
/// along, matching frisket: `Horizontal` places the slots side by side (the
/// divider is a vertical bar), `Vertical` stacks them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

/// The keyboard resize step, as a ratio delta.
const KEY_STEP: f32 = 0.02;

/// The ratio clamp: a fully-dragged divider still leaves a sliver of both
/// slots, so neither side ever vanishes.
const MIN_RATIO: f32 = 0.05;
const MAX_RATIO: f32 = 0.95;

/// The state of a split: which way it divides, where the divider sits, and how
/// thick the seam is. Composable onto an app field via [`lens`](crate::lens).
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub axis: SplitAxis,
    /// The first slot's share, clamped to `0.05..=0.95` by every mutator here;
    /// a caller writing the field directly is clamped at geometry time.
    pub ratio: f32,
    /// Divider thickness (px).
    pub thickness: f32,
    /// Accessible name announced for the divider.
    pub label: String,
}

impl Split {
    /// A split with the divider at `ratio`, a 6px seam.
    pub fn new(axis: SplitAxis, ratio: f32) -> Self {
        Self {
            axis,
            ratio: ratio.clamp(MIN_RATIO, MAX_RATIO),
            thickness: 6.0,
            label: "Pane divider".into(),
        }
    }

    /// Set the accessible name announced for the divider.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// The clamped ratio (a caller may have written the field directly).
    fn clamped(&self) -> f32 {
        self.ratio.clamp(MIN_RATIO, MAX_RATIO)
    }

    /// The two slot rects (`[x, y, w, h]`, container-local) at a container
    /// size. The divider band sits between them; the slots never overlap it.
    pub fn slots(&self, w: f32, h: f32) -> ([f32; 4], [f32; 4]) {
        let t = self.thickness;
        match self.axis {
            SplitAxis::Horizontal => {
                let a = ((w - t) * self.clamped()).round().max(0.0);
                (
                    [0.0, 0.0, a, h],
                    [a + t, 0.0, (w - a - t).max(0.0), h],
                )
            }
            SplitAxis::Vertical => {
                let a = ((h - t) * self.clamped()).round().max(0.0);
                (
                    [0.0, 0.0, w, a],
                    [0.0, a + t, w, (h - a - t).max(0.0)],
                )
            }
        }
    }

    /// The divider band's rect (container-local) at a container size.
    pub fn divider_rect(&self, w: f32, h: f32) -> [f32; 4] {
        let (first, _) = self.slots(w, h);
        match self.axis {
            SplitAxis::Horizontal => [first[2], 0.0, self.thickness, h],
            SplitAxis::Vertical => [0.0, first[3], w, self.thickness],
        }
    }

    /// The ratio that puts the divider's centre at the container-local point
    /// `(x, y)` — the drag, as pure math (the caller owns the ratio, so the
    /// component computes and reports rather than mutating).
    pub fn ratio_at(&self, w: f32, h: f32, x: f32, y: f32) -> f32 {
        let (span, at) = match self.axis {
            SplitAxis::Horizontal => (w - self.thickness, x - self.thickness / 2.0),
            SplitAxis::Vertical => (h - self.thickness, y - self.thickness / 2.0),
        };
        if span > 0.0 {
            (at / span).clamp(MIN_RATIO, MAX_RATIO)
        } else {
            self.clamped()
        }
    }

    /// Move the divider to the container-local point `(x, y)` — [`Self::ratio_at`]
    /// applied, for callers that hold the `Split` directly.
    pub fn drag_to(&mut self, w: f32, h: f32, x: f32, y: f32) {
        self.ratio = self.ratio_at(w, h, x, y);
    }

    /// Move the divider by a ratio delta, clamped — the arrow-key step, exposed
    /// so a caller can drive the same motion from its own shortcut.
    pub fn step(&mut self, delta: f32) {
        self.ratio = (self.clamped() + delta).clamp(MIN_RATIO, MAX_RATIO);
    }
}

/// A split over a [`Split`] and two child views: the children fill the slots,
/// and the divider between them drags and arrow-keys.
///
/// The container is sized by the caller (`w`, `h`) like [`data_grid`]'s
/// viewport: a tiling host knows its area in pixels, and a nested split's
/// child slot size comes from the parent's [`Split::slots`]. Children are
/// whatever the caller supplies — real content for an in-tree consumer, empty
/// slot markers for a host that composites its own surfaces into the slot
/// rects — and they run over the CALLER's state, so the ratio leaves through
/// `on_ratio` (the grid's house style: explicit inputs in, caller-state
/// handlers out) rather than through a lensed mutation. Nested splits are then
/// just child views, each reporting through its own `on_ratio`. Generic over
/// `Action` (the divider emits none; siblings may bubble).
///
/// [`data_grid`]: crate::data_grid
pub fn split<State, Action, OnRatio, First, Second>(
    state: &Split,
    w: f32,
    h: f32,
    on_ratio: OnRatio,
    first: First,
    second: Second,
) -> impl View<State, Action, GenetCtx, Element = GenetElement> + use<State, Action, OnRatio, First, Second>
where
    State: 'static,
    Action: 'static,
    OnRatio: Fn(&mut State, f32) + Clone + 'static,
    First: View<State, Action, GenetCtx, Element = GenetElement>,
    Second: View<State, Action, GenetCtx, Element = GenetElement>,
{
    let (a, b) = state.slots(w, h);
    let d = state.divider_rect(w, h);
    let place = |r: [f32; 4]| {
        format!(
            "position:absolute;left:{}px;top:{}px;width:{}px;height:{}px;",
            r[0], r[1], r[2], r[3]
        )
    };
    let axis = state.axis;
    // The separator's orientation is the bar's own: a vertical bar divides
    // side-by-side slots.
    let orientation = match axis {
        SplitAxis::Horizontal => "vertical",
        SplitAxis::Vertical => "horizontal",
    };
    let divider_origin = (d[0], d[1]);
    let current = state.clamped();
    let for_drag = state.clone();
    let drag_ratio = on_ratio.clone();
    let key_ratio = on_ratio;
    let divider = focusable(on_key(
        on_pointer(
            el::<_, State, Action>("div", ())
                .attr("class", "split-divider")
                .attr("role", "separator")
                .attr("aria-orientation", orientation)
                .attr("aria-label", state.label.clone())
                .attr(
                    "aria-valuenow",
                    ((current * 100.0).round() as i32).to_string(),
                )
                .attr("tabindex", "0")
                .attr("style", place(d)),
            move |s: &mut State, e: PointerEvent| {
                if matches!(e.phase, PointerPhase::Move | PointerPhase::Down) {
                    // Divider-local -> container-local through the origin the
                    // divider was drawn at this build.
                    let r = for_drag.ratio_at(
                        w,
                        h,
                        divider_origin.0 + e.local.0,
                        divider_origin.1 + e.local.1,
                    );
                    drag_ratio(s, r);
                }
            },
        ),
        move |s: &mut State, event| {
            let (dec, inc) = match axis {
                SplitAxis::Horizontal => (NamedKey::ArrowLeft, NamedKey::ArrowRight),
                SplitAxis::Vertical => (NamedKey::ArrowUp, NamedKey::ArrowDown),
            };
            let next = match &event.key {
                Key::Named(k) if *k == dec => Some(current - KEY_STEP),
                Key::Named(k) if *k == inc => Some(current + KEY_STEP),
                Key::Named(NamedKey::Home) => Some(MIN_RATIO),
                Key::Named(NamedKey::End) => Some(MAX_RATIO),
                _ => None,
            };
            if let Some(next) = next {
                key_ratio(s, next.clamp(MIN_RATIO, MAX_RATIO));
                event.prevent_default();
            }
        },
    ));
    el::<_, State, Action>(
        "div",
        (
            el::<_, State, Action>("div", first)
                .attr("class", "split-slot")
                .attr("style", place(a)),
            divider,
            el::<_, State, Action>("div", second)
                .attr("class", "split-slot")
                .attr("style", place(b)),
        ),
    )
    .attr("class", "split")
    .attr(
        "style",
        format!("position:relative;width:{w}px;height:{h}px;"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_and_divider_tile_exactly() {
        let s = Split::new(SplitAxis::Horizontal, 0.5);
        let (a, b) = s.slots(806.0, 600.0);
        let d = s.divider_rect(806.0, 600.0);
        assert_eq!(a[2] + d[2] + b[2], 806.0, "widths must tile the container");
        assert_eq!(a[0] + a[2], d[0], "the divider starts where slot A ends");
        assert_eq!(d[0] + d[2], b[0], "slot B starts where the divider ends");
        let s = Split::new(SplitAxis::Vertical, 0.25);
        let (a, b) = s.slots(400.0, 600.0);
        let d = s.divider_rect(400.0, 600.0);
        assert_eq!(a[3] + d[3] + b[3], 600.0, "heights must tile the container");
    }

    #[test]
    fn drag_follows_the_pointer_and_clamps() {
        let mut s = Split::new(SplitAxis::Horizontal, 0.5);
        s.drag_to(1006.0, 600.0, 253.0, 300.0);
        // 1006 - 6 = 1000 of slot span; pointer at 253 centres the divider at 250.
        assert!((s.ratio - 0.25).abs() < 1e-3, "ratio {}", s.ratio);
        s.drag_to(1006.0, 600.0, -50.0, 300.0);
        assert_eq!(s.ratio, 0.05, "a drag past the edge clamps, never vanishes");
        s.drag_to(1006.0, 600.0, 2000.0, 300.0);
        assert_eq!(s.ratio, 0.95);
    }

    #[test]
    fn step_and_a_stale_ratio_clamp() {
        let mut s = Split::new(SplitAxis::Vertical, 0.94);
        s.step(KEY_STEP);
        assert_eq!(s.ratio, 0.95);
        // A caller wrote the field directly; geometry still clamps.
        s.ratio = 2.0;
        let (a, _) = s.slots(100.0, 106.0);
        assert_eq!(a[3], 95.0, "a stale ratio clamps at geometry time");
        s.step(KEY_STEP);
        assert_eq!(s.ratio, 0.95, "step clamps the stale value before moving");
    }
}
