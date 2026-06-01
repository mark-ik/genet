/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Serval's `engine_observables_api` consumer-side surface.
//!
//! [`ServalLaneView`] bundles a borrowed `(dom, styles, fragments)`
//! triple and implements [`FragmentQuery`] + [`InteractionQuery`] over
//! it. The internal plane storage (FragmentPlane / StylePlane) stays
//! serval-layout's implementation detail; consumers (mere-host,
//! Apparatus, Hekate) speak only the trait surface.
//!
//! ## Probe v1 scope (2026-05-18)
//!
//! - `FragmentQuery::hit_test` walks the DOM in **paint order**
//!   (pre-order document order; the same order `paint_emit` produces
//!   its command stream) and keeps the **last** containing rect — that
//!   matches "topmost in paint order is later in the list."
//! - `FragmentQuery::box_model` returns the same rect for content /
//!   padding / border / margin because the cascade doesn't yet apply
//!   real CSS padding/border/margin. Once real stylesheets land, this
//!   is the seam where `ComputedValues::margin/padding/border` shapes
//!   each box.
//! - Anchor + selection methods return empty/None — no anchor index
//!   or selection state in the probe.
//! - All `InteractionQuery` methods return empty/None — no focus,
//!   selection, or affordance machinery yet. The traits define the
//!   contract; impls land alongside real interaction wiring.
//!
//! The `SourceNodeId ↔ D::NodeId` round-trip uses `LayoutDom::opaque_id`
//! (forward) + a DOM walk (reverse). The reverse walk is O(n) per
//! `box_model` call — acceptable for the probe, fixable with a
//! reverse-index `FxHashMap<u64, D::NodeId>` cached on the view when
//! a consumer pulls on perf.

use std::hash::Hash;

use engine_observables_api::{
    Affordance, BoxModel, FragmentHit, FragmentQuery, InteractionQuery, Point, Rect, Selection,
    SourceNodeId, SourceRange,
};
use layout_dom_api::LayoutDom;

use crate::fragment::FragmentPlane;
use crate::paint_emit::{clips_overflow, ScrollOffsets};
use crate::style::StylePlane;

/// Borrowed view over Serval's planes, exposing the engine_observables_api
/// query surface.
///
/// Constructed cheaply after a layout pass: borrow the dom + planes
/// (the host typically owns them in an `Arc`/`Rc` per tile, and hands
/// references to the lane view per query).
pub struct ServalLaneView<'a, D: LayoutDom> {
    pub dom: &'a D,
    pub styles: &'a StylePlane<D::NodeId>,
    pub fragments: &'a FragmentPlane<D::NodeId>,
    /// Epoch — rolled by the producer whenever the planes regenerate.
    /// Consumers cache observations against this.
    pub generation: u64,
    /// Per-node scroll offsets, for clip-aware hit-testing: inside a scroll
    /// container the query point maps through `+offset` (the inverse of paint's
    /// `-offset` content translate). `None` ⇒ nothing scrolls. The host (which
    /// owns the offsets) supplies the same map it passes to paint.
    scroll_offsets: Option<&'a ScrollOffsets<D::NodeId>>,
}

impl<'a, D: LayoutDom> ServalLaneView<'a, D> {
    pub fn new(
        dom: &'a D,
        styles: &'a StylePlane<D::NodeId>,
        fragments: &'a FragmentPlane<D::NodeId>,
    ) -> Self {
        Self {
            dom,
            styles,
            fragments,
            generation: 0,
            scroll_offsets: None,
        }
    }

    pub fn with_generation(mut self, gen_id: u64) -> Self {
        self.generation = gen_id;
        self
    }

    /// Supply per-node scroll offsets so hit-testing maps the query point
    /// through scrolled containers (and clips to overflow boxes). Pass the same
    /// map handed to paint.
    pub fn with_scroll_offsets(mut self, offsets: &'a ScrollOffsets<D::NodeId>) -> Self {
        self.scroll_offsets = Some(offsets);
        self
    }

    /// Reverse-lookup a `D::NodeId` for a given `SourceNodeId`.
    /// O(n) over the DOM; acceptable for probe-stage. Cf. module doc.
    fn find_by_source_id(&self, source_id: SourceNodeId) -> Option<D::NodeId>
    where
        D::NodeId: Copy + Eq + Hash,
    {
        let mut queue = vec![self.dom.document()];
        while let Some(id) = queue.pop() {
            if self.dom.opaque_id(id) == source_id.0 {
                return Some(id);
            }
            queue.extend(self.dom.dom_children(id));
        }
        None
    }
}

impl<'a, D: LayoutDom> FragmentQuery for ServalLaneView<'a, D>
where
    D::NodeId: Copy + Eq + Hash,
{
    type FragmentId = SourceNodeId;

    fn generation_id(&self) -> u64 {
        self.generation
    }

    fn hit_test(&self, point: Point) -> Option<FragmentHit<Self::FragmentId>> {
        let mut hit: Option<FragmentHit<SourceNodeId>> = None;
        walk_for_hit(
            self.dom,
            self.styles,
            self.fragments,
            self.scroll_offsets,
            self.dom.document(),
            Point::new(0.0, 0.0),
            point,
            &mut hit,
        );
        hit
    }

    fn box_model(&self, source_id: SourceNodeId) -> Option<BoxModel> {
        let node = self.find_by_source_id(source_id)?;
        // Walk to absolute origin (taffy::Layout.location is parent-relative).
        let origin = absolute_origin(self.dom, self.fragments, node)?;
        let layout = self.fragments.rect_of(node)?;

        // `taffy::Layout`'s position+size is the BORDER box. Inset by
        // border widths to get padding box; inset that by padding to
        // get content box; outset border by margin to get margin box.
        // When the cascade hasn't been applied (hand-rolled styles
        // with no margin/padding/border), all four collapse to the
        // border box — matches CSS semantics for an unstyled element.
        let border = Rect::new(origin, layout.size.width, layout.size.height);
        let padding = inset_rect(border, layout.border);
        let content = inset_rect(padding, layout.padding);
        let margin = outset_rect(border, layout.margin);

        Some(BoxModel { content, padding, border, margin })
    }

    fn fragments_for_anchor<'b>(
        &'b self,
        _anchor: &str,
    ) -> Box<dyn Iterator<Item = Self::FragmentId> + 'b> {
        // No anchor index in probe; cascade-driven id/name extraction
        // lands when stylesheets apply.
        Box::new(std::iter::empty())
    }

    fn text_range_for_fragment(&self, _fragment: Self::FragmentId) -> Option<SourceRange> {
        // Source-range tracking not wired through construct yet.
        None
    }

    fn rects_for_selection(&self, _range: SourceRange) -> Vec<Rect> {
        // Line-box info not exposed by current FragmentPlane shape;
        // populates once parley line boxes thread through to fragments.
        Vec::new()
    }
}

impl<'a, D: LayoutDom> InteractionQuery for ServalLaneView<'a, D>
where
    D::NodeId: Copy + Eq + Hash,
{
    fn focus_target(&self) -> Option<SourceNodeId> {
        None
    }

    fn selection(&self) -> Option<Selection> {
        None
    }

    fn affordances_at(&self, _point: Point) -> Vec<Affordance> {
        Vec::new()
    }

    fn activation_target(&self, _point: Point) -> Option<SourceNodeId> {
        None
    }
}

/// Walk in paint order accumulating origin; if `point` falls in this node's
/// fragment, record the hit (overwriting any earlier hit — "topmost in paint
/// order is later in the walk"). Clip- and scroll-aware, mirroring paint:
///   * an overflow container clips its descendants to its padding box, so a
///     point outside that box skips the subtree (no leak onto elements below a
///     scrolled box);
///   * a scroll container's descendants are queried at `point + offset` (the
///     inverse of paint's `-offset` content translate).
#[allow(clippy::too_many_arguments)]
fn walk_for_hit<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    scroll_offsets: Option<&ScrollOffsets<D::NodeId>>,
    id: D::NodeId,
    parent_origin: Point,
    point: Point,
    out: &mut Option<FragmentHit<SourceNodeId>>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let layout = fragments.rect_of(id);
    let origin = if let Some(l) = layout {
        Point::new(parent_origin.x + l.location.x, parent_origin.y + l.location.y)
    } else {
        parent_origin
    };

    if let Some(l) = layout {
        let rect = Rect::new(origin, l.size.width, l.size.height);
        if rect.contains(point) {
            *out = Some(FragmentHit {
                fragment: SourceNodeId(dom.opaque_id(id)),
                source_node: SourceNodeId(dom.opaque_id(id)),
                local_point: Point::new(point.x - origin.x, point.y - origin.y),
            });
        }

        // Clip: an overflow container clips its descendants to its padding box.
        // A point outside that box can't hit them — skip the subtree (this is
        // what stops a scrolled box's clicks leaking onto the element below it).
        if clips_overflow(styles, id) {
            let pad = Rect::new(
                Point::new(origin.x + l.border.left, origin.y + l.border.top),
                l.size.width - l.border.left - l.border.right,
                l.size.height - l.border.top - l.border.bottom,
            );
            if !pad.contains(point) {
                return;
            }
        }
    }

    // Scroll: descendants of a scroll container are painted translated by
    // `-offset`, so query them at `point + offset`.
    let child_point = match scroll_offsets.and_then(|m| m.get(&id)) {
        Some(&(ox, oy)) => Point::new(point.x + ox, point.y + oy),
        None => point,
    };

    for child in dom.dom_children(id) {
        walk_for_hit(dom, styles, fragments, scroll_offsets, child, origin, child_point, out);
    }
}

/// Shrink a rect by the given four insets (top/right/bottom/left).
/// Used to derive padding-box from border-box, content-box from
/// padding-box. Negative dimensions clamp to zero — easier than
/// asserting layout sanity.
fn inset_rect(rect: Rect, insets: taffy::Rect<f32>) -> Rect {
    let new_w = (rect.width - insets.left - insets.right).max(0.0);
    let new_h = (rect.height - insets.top - insets.bottom).max(0.0);
    Rect::new(
        Point::new(rect.origin.x + insets.left, rect.origin.y + insets.top),
        new_w,
        new_h,
    )
}

/// Grow a rect by the given four outsets. Used to derive margin-box
/// from border-box.
fn outset_rect(rect: Rect, outsets: taffy::Rect<f32>) -> Rect {
    Rect::new(
        Point::new(rect.origin.x - outsets.left, rect.origin.y - outsets.top),
        rect.width + outsets.left + outsets.right,
        rect.height + outsets.top + outsets.bottom,
    )
}

/// Walk from the document root to `target`, accumulating origins.
/// Returns the absolute origin of `target`, or None if not reachable.
/// O(n) — adequate for probe; a cached parent_id map would make this
/// O(depth).
fn absolute_origin<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    target: D::NodeId,
) -> Option<Point>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn recurse<D>(
        dom: &D,
        fragments: &FragmentPlane<D::NodeId>,
        id: D::NodeId,
        target: D::NodeId,
        parent_origin: Point,
    ) -> Option<Point>
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        let layout = fragments.rect_of(id);
        let origin = if let Some(l) = layout {
            Point::new(parent_origin.x + l.location.x, parent_origin.y + l.location.y)
        } else {
            parent_origin
        };
        if id == target {
            return Some(origin);
        }
        for child in dom.dom_children(id) {
            if let Some(p) = recurse(dom, fragments, child, target, origin) {
                return Some(p);
            }
        }
        None
    }
    recurse(dom, fragments, dom.document(), target, Point::new(0.0, 0.0))
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use serval_static_dom::{StaticDocument, StaticNodeId};

    use super::*;
    use crate::adapter::NodeRef;
    use crate::image_decode::ImagePlane;
    use crate::layout::layout;

    /// Cascade-driven style plane sizing block elements to a fixed
    /// 200×50 with no spacing — the box tree reads `ComputedValues`, so
    /// these tests now drive layout through the real cascade rather than
    /// hand-built Taffy styles. No margin/padding/border keeps the four
    /// box-model rects coincident (what the box_model tests rely on).
    fn build_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        crate::cascade::run_cascade(
            document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &["p, div { display: block; width: 200px; height: 50px; margin: 0; padding: 0; border: 0; }"],
            None,
        );
        plane
    }

    fn find_element<'a>(
        start: NodeRef<'a, StaticDocument>,
        local: html5ever::LocalName,
    ) -> Option<NodeRef<'a, StaticDocument>> {
        let mut queue = vec![start];
        while let Some(node) = queue.pop() {
            if let Some(name) = node.dom().element_name(node.id()) {
                if name.local == local {
                    return Some(node);
                }
            }
            queue.extend(node.dom_children());
        }
        None
    }

    #[test]
    fn hit_test_returns_topmost_fragment_containing_point() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        // Compute a point known to be inside <p>'s rect (avoids
        // depending on html5ever's auto-inserted <head> stacking the
        // body lower than expected).
        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");
        let p_origin = absolute_origin(&document, &fragments, p.id())
            .expect("<p> has an origin");
        let p_layout = fragments.rect_of(p.id()).expect("<p> has a fragment");
        let point = Point::new(
            p_origin.x + p_layout.size.width * 0.5,
            p_origin.y + p_layout.size.height * 0.5,
        );

        let hit = view.hit_test(point).expect("hit something");

        // <p> is the deepest containing fragment at this point — it
        // wins over any ancestor in pre-order topmost semantics.
        let expected_opaque = document.opaque_id(p.id());
        assert_eq!(
            hit.source_node.0, expected_opaque,
            "expected hit on <p> (opaque_id {expected_opaque}), got opaque_id {}",
            hit.source_node.0
        );
    }

    #[test]
    fn hit_test_misses_outside_layout() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        // Way outside the viewport — no fragment contains it.
        assert!(view.hit_test(Point::new(10_000.0, 10_000.0)).is_none());
    }

    #[test]
    fn box_model_returns_rect_for_known_source_id() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");
        let p_source = SourceNodeId(document.opaque_id(p.id()));

        let bm = view.box_model(p_source).expect("<p> has a box_model");
        assert!(bm.border.width > 0.0);
        assert!(bm.border.height > 0.0);
        // Hand-rolled style plane with no margin/padding/border: all
        // four boxes coincide. Cascade-driven styles distinguish them
        // (see box_model_returns_distinct_rects_with_cascade_styling).
        assert_eq!(bm.content, bm.padding);
        assert_eq!(bm.padding, bm.border);
        assert_eq!(bm.border, bm.margin);
    }

    /// Cascade-driven layout produces distinct content/padding/border/
    /// margin rects. The stylesheet sets `<p>` to width 100, height 50,
    /// border 4px, padding 8px, margin 16px; box_model must return
    /// rects whose deltas match.
    #[test]
    fn box_model_returns_distinct_rects_with_cascade_styling() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse(
            "<html><body><p>x</p></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "p { display: block; width: 100px; height: 50px; \
                    border: 4px solid black; padding: 8px; margin: 16px; }",
            ],
            None,
        );

        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");
        let p_source = SourceNodeId(document.opaque_id(p.id()));
        let bm = view.box_model(p_source).expect("<p> has a box_model");

        // Border-box = content (100x50) + padding (8 each side) +
        // border (4 each side). So 100 + 16 + 8 = 124 wide, 50 + 16 +
        // 8 = 74 tall.
        assert!(
            (bm.border.width - 124.0).abs() < 0.1,
            "border width: {}",
            bm.border.width
        );
        assert!(
            (bm.border.height - 74.0).abs() < 0.1,
            "border height: {}",
            bm.border.height
        );

        // Padding-box = border-box inset by border (4 each side).
        // 124 - 8 = 116, 74 - 8 = 66.
        assert!(
            (bm.padding.width - 116.0).abs() < 0.1,
            "padding width: {}",
            bm.padding.width
        );
        assert!(
            (bm.padding.height - 66.0).abs() < 0.1,
            "padding height: {}",
            bm.padding.height
        );

        // Content-box = padding-box inset by padding (8 each side).
        // 116 - 16 = 100, 66 - 16 = 50 — matches the CSS width/height.
        assert!(
            (bm.content.width - 100.0).abs() < 0.1,
            "content width: {}",
            bm.content.width
        );
        assert!(
            (bm.content.height - 50.0).abs() < 0.1,
            "content height: {}",
            bm.content.height
        );

        // Margin-box = border-box outset by margin (16 each side).
        // 124 + 32 = 156, 74 + 32 = 106.
        assert!(
            (bm.margin.width - 156.0).abs() < 0.1,
            "margin width: {}",
            bm.margin.width
        );
        assert!(
            (bm.margin.height - 106.0).abs() < 0.1,
            "margin height: {}",
            bm.margin.height
        );
    }

    #[test]
    fn box_model_returns_none_for_unknown_source_id() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        assert!(view.box_model(SourceNodeId(0xDEAD_BEEF)).is_none());
    }

    #[test]
    fn interaction_query_returns_empty_in_probe() {
        let document = StaticDocument::parse("<html><body></body></html>");
        let styles = build_style_plane(&document);
        let (fragments, _, _) = layout(
            &document,
            &styles,
            &ImagePlane::new(),
            taffy::Size {
                width: taffy::AvailableSpace::Definite(800.0),
                height: taffy::AvailableSpace::Definite(600.0),
            },
        );
        let view = ServalLaneView::new(&document, &styles, &fragments);

        assert!(view.focus_target().is_none());
        assert!(view.selection().is_none());
        assert!(view.affordances_at(Point::new(10.0, 10.0)).is_empty());
        assert!(view.activation_target(Point::new(10.0, 10.0)).is_none());
    }

    /// Collect every `<div>` in document (pre-)order. The clip test builds a
    /// three-div layout (`box`, `inner`, `below`) and indexes the result.
    fn collect_divs<'a>(
        start: NodeRef<'a, StaticDocument>,
        out: &mut Vec<NodeRef<'a, StaticDocument>>,
    ) {
        if let Some(name) = start.dom().element_name(start.id()) {
            if name.local == local_name!("div") {
                out.push(start);
            }
        }
        for child in start.dom_children() {
            collect_divs(child, out);
        }
    }

    /// Hit-testing respects overflow clips and per-node scroll offsets:
    /// a `box` (overflow:scroll, 40px tall) holds an `inner` taller than itself
    /// and is followed by a `below` sibling.
    ///
    ///   * A click *below* the box hits `below`, not the (clipped) `inner` —
    ///     without clip awareness `inner`'s 200px-tall fragment would swallow it.
    ///   * With the box scrolled down, a click at its top hits `inner` at the
    ///     scrolled-in content offset (the click maps through `point + offset`).
    #[test]
    fn hit_test_is_clip_and_scroll_aware() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse(
            "<html><body>\
                <div class=\"box\"><div class=\"inner\">tall</div></div>\
                <div class=\"below\">b</div>\
            </body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(200.0, 600.0),
            &[
                "html, body { display: block; margin: 0; padding: 0; }",
                "div { display: block; margin: 0; padding: 0; border: 0; }",
                ".box { overflow: scroll; width: 100px; height: 40px; }",
                ".inner { height: 200px; }",
                ".below { height: 30px; }",
            ],
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(200.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);

        let mut divs = Vec::new();
        collect_divs(NodeRef::document(&document), &mut divs);
        assert_eq!(divs.len(), 3, "box, inner, below");
        let id_of = |n: &NodeRef<StaticDocument>| SourceNodeId(document.opaque_id(n.id()));
        let (box_id, inner_id, below_id) =
            (id_of(&divs[0]), id_of(&divs[1]), id_of(&divs[2]));
        let _ = box_id;

        // (1) Clip: a point below the 40px box hits `below`, NOT the 200px
        // `inner` clipped inside the box.
        let view = ServalLaneView::new(&document, &styles, &fragments);
        let hit = view.hit_test(Point::new(5.0, 50.0)).expect("hits below");
        assert_eq!(hit.source_node, below_id, "click below the box hits `below`");

        // (2) Scroll: with the box scrolled down 80px, a click at its top maps
        // through the offset and hits `inner` at content-y ≈ 85.
        let mut offsets = ScrollOffsets::<StaticNodeId>::default();
        offsets.insert(divs[0].id(), (0.0, 80.0));
        let scrolled =
            ServalLaneView::new(&document, &styles, &fragments).with_scroll_offsets(&offsets);
        let hit = scrolled.hit_test(Point::new(5.0, 5.0)).expect("hits inner");
        assert_eq!(hit.source_node, inner_id, "click in the scrolled box hits `inner`");
        assert!(
            (hit.local_point.y - 85.0).abs() < 0.1,
            "local point maps through the scroll offset: {}",
            hit.local_point.y
        );
    }
}
