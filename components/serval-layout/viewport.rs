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

use crate::fragment::FragmentPlane;
use crate::paint_emit::{clips_overflow, generates_box, is_fixed, primary_cv};
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

    /// Clamp a desired document scroll to what the viewport can actually reach: an
    /// axis the viewport does not scroll (propagated `overflow: hidden`/`clip`) pins
    /// at 0, and a scrollable axis clamps to `[0, range]` (the scrollable-overflow
    /// extent from [`document_scroll_range`]). The one shared clamp every viewport
    /// owner (the [`IncrementalLayout`](crate::IncrementalLayout) session, pelt's
    /// static viewer) routes its wheel / key default action through, so there is no
    /// second hand-rolled copy (scope doc rule 5).
    pub fn clamp_scroll(&self, desired: (f32, f32), range: (f32, f32)) -> (f32, f32) {
        let x = if self.scrolls_x() { desired.0.clamp(0.0, range.0) } else { 0.0 };
        let y = if self.scrolls_y() { desired.1.clamp(0.0, range.1) } else { 0.0 };
        (x, y)
    }
}

/// A keyboard scroll default action (scope doc rule 5): the document scroll an
/// arrow / `PageUp` / `PageDown` / `Home` / `End` key performs when focus is not in
/// an editable. Resolved against the viewport (page size) and the scroll range by
/// [`IncrementalLayout::scroll_for_key`](crate::IncrementalLayout::scroll_for_key),
/// so a host only maps its key event onto this and gates on focus. `Space` /
/// `Shift+Space` map to `PageDown` / `PageUp` at the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollKey {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
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

/// The document's maximum scroll offset in device px (scope doc rule 4): how far
/// the viewport can scroll before its scrollable-overflow region is exhausted
/// (CSS Overflow §scrollable). Per axis it is `max(0, content_extent -
/// viewport_size)`, where the content extent is the far edge of the union of
/// in-flow and `absolute` fragments. A `position: fixed` box is viewport-anchored,
/// so it and its subtree do not extend the range; an overflow-clip container
/// bounds its descendants to its own box. The host clamps the viewport scroll to
/// this so the page cannot scroll past its content.
///
/// Bounds come from the spec, not "content height": this reads the retained
/// fragment plane in DOM order (the same origin accumulation paint and hit-test
/// use), so root margin and the abs-pos containing-block subtleties inherit those
/// passes' fidelity rather than a separate height heuristic.
pub fn document_scroll_range<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    size: DeviceIntSize,
) -> (f32, f32)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut extent = (0.0f32, 0.0f32);
    extend_scrollable(dom, styles, fragments, dom.document(), (0.0, 0.0), &mut extent);
    ((extent.0 - size.width as f32).max(0.0), (extent.1 - size.height as f32).max(0.0))
}

/// Accumulate the far (right, bottom) edge of `id`'s fragment and its scrollable
/// descendants into `extent`, in absolute coords (`parent_origin` accumulated
/// through the DOM, as paint/hit-test do). Skips `position: fixed` subtrees
/// (viewport-anchored) and does not descend past an overflow-clip container
/// (its own box already bounds them).
fn extend_scrollable<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    parent_origin: (f32, f32),
    extent: &mut (f32, f32),
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let cv = primary_cv(styles, id);
    // A fixed box attaches to the viewport: it (and its subtree) never scrolls, so
    // it does not extend the document's scroll range.
    if cv.as_deref().is_some_and(is_fixed) {
        return;
    }
    let origin = match fragments.rect_of(id) {
        Some(l) => {
            let ox = parent_origin.0 + l.location.x;
            let oy = parent_origin.1 + l.location.y;
            extent.0 = extent.0.max(ox + l.size.width);
            extent.1 = extent.1.max(oy + l.size.height);
            (ox, oy)
        },
        None => parent_origin,
    };
    // An overflow-clip container bounds its descendants to its own box (counted
    // above); their fragments cannot extend the range past it.
    if cv.as_deref().is_some_and(clips_overflow) {
        return;
    }
    for child in dom.dom_children(id) {
        extend_scrollable(dom, styles, fragments, child, origin, extent);
    }
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

    /// Lay out `html` cascaded with `sheet` at `w`x`h` and return the document's
    /// maximum scroll offset.
    fn scroll_range_of(html: &str, sheet: &str, w: f32, h: f32) -> (f32, f32) {
        use crate::image_decode::ImagePlane;
        use crate::layout::layout;

        let document = StaticDocument::parse(html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(&document, &mut styles, euclid::default::Size2D::new(w, h), &[sheet], None);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(w),
            height: taffy::AvailableSpace::Definite(h),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        document_scroll_range(&document, &styles, &fragments, DeviceIntSize::new(w as i32, h as i32))
    }

    /// The scroll range is the content overflow beyond the viewport (rule 4):
    /// 2000px of content in a 600px viewport scrolls 1400px vertically, and
    /// 800px-wide content in an 800px viewport does not scroll horizontally.
    #[test]
    fn scroll_range_is_content_overflow_beyond_the_viewport() {
        let range = scroll_range_of(
            "<html><body><div class=\"tall\"></div></body></html>",
            "html, body, div { display: block; margin: 0; } .tall { height: 2000px; }",
            800.0,
            600.0,
        );
        assert!(
            (range.1 - 1400.0).abs() < 1.0,
            "vertical range is content(2000) - viewport(600): {}",
            range.1,
        );
        assert_eq!(range.0, 0.0, "content is not wider than the viewport");
    }

    /// A `position: fixed` box is viewport-anchored, so it does not extend the
    /// document scroll range — a tall fixed box over a short document still
    /// scrolls nowhere.
    #[test]
    fn fixed_box_does_not_extend_scroll_range() {
        let range = scroll_range_of(
            "<html><body><div class=\"short\"></div><div class=\"fixed\"></div></body></html>",
            "html, body, div { display: block; margin: 0; } \
             .short { height: 100px; } \
             .fixed { position: fixed; top: 0; left: 0; width: 50px; height: 3000px; }",
            800.0,
            600.0,
        );
        assert_eq!(range, (0.0, 0.0), "a fixed box does not extend the document scroll range");
    }

    /// An overflow-clip container bounds the scroll range to its own box: a 40px
    /// `overflow: hidden` box holding a 2000px child contributes only its 40px,
    /// not the clipped child's 2000px.
    #[test]
    fn overflow_clip_container_bounds_the_scroll_range() {
        let range = scroll_range_of(
            "<html><body><div class=\"box\"><div class=\"inner\"></div></div></body></html>",
            "html, body, div { display: block; margin: 0; } \
             .box { overflow: hidden; width: 100px; height: 40px; } \
             .inner { height: 2000px; }",
            800.0,
            600.0,
        );
        assert_eq!(range.1, 0.0, "the clipped 2000px child does not extend the scroll range");
    }

    /// `clamp_scroll` pins non-scrollable axes at 0 and clamps scrollable axes to
    /// `[0, range]` — the shared default-action clamp.
    #[test]
    fn clamp_scroll_respects_overflow_and_range() {
        let scrollable = Viewport {
            size: DeviceIntSize::new(800, 600),
            scroll: (0.0, 0.0),
            overflow: (Overflow::Visible, Overflow::Visible),
        };
        assert_eq!(
            scrollable.clamp_scroll((50.0, 5000.0), (0.0, 1400.0)),
            (0.0, 1400.0),
            "an over-scroll clamps to the range (x range 0 pins x at 0)",
        );
        assert_eq!(
            scrollable.clamp_scroll((-10.0, -10.0), (0.0, 1400.0)),
            (0.0, 0.0),
            "no negative scroll",
        );
        let hidden = Viewport { overflow: (Overflow::Hidden, Overflow::Hidden), ..scrollable };
        assert_eq!(
            hidden.clamp_scroll((50.0, 500.0), (1000.0, 1400.0)),
            (0.0, 0.0),
            "overflow:hidden axes do not scroll regardless of range",
        );
    }
}
