/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The document viewport: a first-class per-document object for the root/viewport
//! special rules (`docs/2026-06-12_viewport_root_standards_scope.md`, rule 1).
//!
//! An engine built element-first has no "viewport" — it threads a bare
//! `(width, height)` through layout and paint, and so misses the family of CSS
//! rules where the root element and the viewport behave specially. The first of
//! those is **document scroll**: a page taller than the window scrolls with zero
//! CSS, because the root element's `overflow` is *propagated to the viewport*
//! (CSS Overflow §3.3) and the viewport scrolls its overflowing content. serval
//! already implements the sibling propagation rule for *backgrounds*
//! ([`paint_emit::emit_canvas_background`](crate::paint_emit)); this object does
//! "the same for overflow" — the rule of engagement is that the spec's mechanism
//! lives here, in the engine, never faked host-side by keying the root element in
//! [`ScrollOffsets`](crate::ScrollOffsets).
//!
//! A1 (this module's first slice) introduces the object and the propagation read.
//! Paint translation by the viewport scroll, `position: fixed` attachment, the
//! hit-test offset, and the `IncrementalLayout` slot follow.

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use paint_list_api::DeviceIntSize;
use style::values::computed::Overflow;

use crate::paint_emit::{generates_box, primary_cv};
use crate::style::StylePlane;

/// The per-document viewport (the initial containing block): its size, the
/// document scroll offset, and the `overflow` propagated to it from the root
/// (else body). Held beside [`ScrollOffsets`](crate::ScrollOffsets) — which keys
/// *element* overflow containers — never as a faked root-element scroll key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// The viewport size in device px (the initial containing block).
    pub size: DeviceIntSize,
    /// The document scroll offset in device px; in-flow content paints translated
    /// by `-scroll`. `position: fixed` content is exempt (it attaches here).
    pub scroll: (f32, f32),
    /// The `(x, y)` overflow propagated to the viewport from the root element
    /// (CSS Overflow §3.3). Determines whether the document scrolls.
    pub overflow: (Overflow, Overflow),
}

impl Viewport {
    /// Build the viewport for `dom` at `size`, computing the propagated overflow;
    /// the scroll offset starts at the origin.
    pub fn for_document<D>(dom: &D, styles: &StylePlane<D::NodeId>, size: DeviceIntSize) -> Self
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        Self {
            size,
            scroll: (0.0, 0.0),
            overflow: propagated_overflow(dom, styles),
        }
    }

    /// Whether the document scrolls horizontally — the viewport scrolls its
    /// overflow unless the propagated value clips it (`hidden` / `clip`).
    pub fn scrolls_x(&self) -> bool {
        is_scrollable(self.overflow.0)
    }

    /// Whether the document scrolls vertically (see [`scrolls_x`](Self::scrolls_x)).
    pub fn scrolls_y(&self) -> bool {
        is_scrollable(self.overflow.1)
    }
}

/// `visible` / `auto` / `scroll` all let the viewport reach its overflowing
/// content (a plain page scrolls under `visible`); only `hidden` (and `clip`)
/// suppress document scroll.
fn is_scrollable(o: Overflow) -> bool {
    matches!(o, Overflow::Visible | Overflow::Auto | Overflow::Scroll)
}

/// The overflow propagated to the viewport (CSS Overflow §3.3): the root
/// element's, except when the root's is `visible` on **both** axes, in which case
/// the HTML `<body>`'s is used. This is the exact root→else→body source choice the
/// canvas-background propagation makes ([`paint_emit::emit_canvas_background`](crate::paint_emit)).
/// Falls back to `visible` (a scrollable viewport) when no box-generating root
/// exists — a `display: none` root hides the document, so there is nothing to
/// scroll.
fn propagated_overflow<D>(dom: &D, styles: &StylePlane<D::NodeId>) -> (Overflow, Overflow)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::local_name;

    let default = (Overflow::Visible, Overflow::Visible);
    // Root = the first element child of the document node (as the background
    // propagation finds it).
    let Some(root) = dom
        .dom_children(dom.document())
        .find(|&c| dom.kind(c) == NodeKind::Element)
    else {
        return default;
    };
    let Some(root_cv) = primary_cv(styles, root) else {
        return default;
    };
    if !generates_box(&root_cv) {
        return default;
    }
    let root_box = root_cv.get_box();
    let (rx, ry) = (root_box.overflow_x, root_box.overflow_y);
    // The root propagates its own overflow unless it is `visible` on both axes;
    // then the body does (the §3.3 body→viewport special case, the sibling of the
    // canvas-background body fallback).
    if !matches!(rx, Overflow::Visible) || !matches!(ry, Overflow::Visible) {
        return (rx, ry);
    }
    let body = dom.dom_children(root).find(|&c| {
        dom.kind(c) == NodeKind::Element
            && dom.element_name(c).is_some_and(|q| q.local == local_name!("body"))
    });
    if let Some(body_cv) = body.and_then(|b| primary_cv(styles, b)) {
        if generates_box(&body_cv) {
            let bb = body_cv.get_box();
            return (bb.overflow_x, bb.overflow_y);
        }
    }
    default
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::run_cascade;
    use serval_static_dom::{StaticDocument, StaticNodeId};

    const DOC: &str = "<html><body><div></div></body></html>";

    /// Cascade `DOC` with `sheet` and return the overflow propagated to the
    /// viewport.
    fn propagated(sheet: &str) -> (Overflow, Overflow) {
        let document = StaticDocument::parse(DOC);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::default::Size2D::new(800.0, 600.0),
            &[sheet],
            None,
        );
        propagated_overflow(&document, &styles)
    }

    const BLOCK: &str = "html, body, div { display: block; }";

    /// A plain document scrolls: with no overflow styling the viewport inherits
    /// `visible`/`visible`, which is scrollable — the default page scroll, not a
    /// clip.
    #[test]
    fn plain_document_viewport_is_visible_and_scrollable() {
        let overflow = propagated(BLOCK);
        assert_eq!(overflow, (Overflow::Visible, Overflow::Visible));
        let vp = Viewport { size: DeviceIntSize::new(800, 600), scroll: (0.0, 0.0), overflow };
        assert!(vp.scrolls_x() && vp.scrolls_y(), "a plain page scrolls");
    }

    /// `overflow: hidden` on the root propagates to the viewport and disables
    /// document scroll (§3.3 root propagation).
    #[test]
    fn root_overflow_hidden_disables_viewport_scroll() {
        let overflow = propagated(&format!("{BLOCK} html {{ overflow: hidden; }}"));
        assert_eq!(overflow.1, Overflow::Hidden);
        let vp = Viewport { size: DeviceIntSize::new(800, 600), scroll: (0.0, 0.0), overflow };
        assert!(!vp.scrolls_y(), "overflow:hidden on the root stops the document scrolling");
    }

    /// When the root is `visible`, the body's overflow propagates instead — the
    /// §3.3 body→viewport special case, sibling of the canvas-background body
    /// fallback.
    #[test]
    fn body_overflow_propagates_when_root_is_visible() {
        let overflow = propagated(&format!("{BLOCK} body {{ overflow: scroll; }}"));
        assert_eq!(overflow.1, Overflow::Scroll, "root visible → the body's overflow is the viewport's");
    }

    /// A non-`visible` root keeps its own overflow; the body is not consulted.
    #[test]
    fn root_overflow_wins_when_root_not_visible() {
        let overflow = propagated(&format!(
            "{BLOCK} html {{ overflow: scroll; }} body {{ overflow: hidden; }}"
        ));
        assert_eq!(
            overflow.1,
            Overflow::Scroll,
            "a non-visible root propagates its own overflow, not the body's",
        );
    }
}
