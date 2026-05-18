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
        }
    }

    pub fn with_generation(mut self, gen_id: u64) -> Self {
        self.generation = gen_id;
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
            self.fragments,
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
        let rect = Rect::new(origin, layout.size.width, layout.size.height);
        // Probe stage: padding/border/margin not yet driven from
        // ComputedValues — every box collapses to the border-box rect.
        Some(BoxModel {
            content: rect,
            padding: rect,
            border: rect,
            margin: rect,
        })
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

/// Walk in paint order accumulating origin; if `point` falls in this
/// node's fragment, record the hit (overwriting any earlier hit —
/// "topmost in paint order is later in the walk").
fn walk_for_hit<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
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
    }

    for child in dom.dom_children(id) {
        walk_for_hit(dom, fragments, child, origin, point, out);
    }
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
    use taffy::prelude::*;

    use super::*;
    use crate::adapter::NodeRef;
    use crate::layout::layout;
    use crate::style::StyleEntry;

    fn build_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        let root = NodeRef::document(document);
        let mut queue = vec![root];
        while let Some(node) = queue.pop() {
            if document.element_name(node.id()).is_some() {
                plane.insert(
                    node.id(),
                    StyleEntry {
                        taffy: Style {
                            display: Display::Block,
                            size: Size {
                                width: length(200.0),
                                height: length(50.0),
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            }
            queue.extend(node.dom_children());
        }
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
        let (fragments, _) = layout(&document, &styles, viewport);
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
        let (fragments, _) = layout(&document, &styles, viewport);
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
        let (fragments, _) = layout(&document, &styles, viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        let p = find_element(NodeRef::document(&document), local_name!("p"))
            .expect("<p> exists");
        let p_source = SourceNodeId(document.opaque_id(p.id()));

        let bm = view.box_model(p_source).expect("<p> has a box_model");
        assert!(bm.border.width > 0.0);
        assert!(bm.border.height > 0.0);
        // Probe collapse: all four boxes coincide until cascade applies
        // real padding/border/margin.
        assert_eq!(bm.content, bm.padding);
        assert_eq!(bm.padding, bm.border);
        assert_eq!(bm.border, bm.margin);
    }

    #[test]
    fn box_model_returns_none_for_unknown_source_id() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _) = layout(&document, &styles, viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        assert!(view.box_model(SourceNodeId(0xDEAD_BEEF)).is_none());
    }

    #[test]
    fn interaction_query_returns_empty_in_probe() {
        let document = StaticDocument::parse("<html><body></body></html>");
        let styles = build_style_plane(&document);
        let (fragments, _) = layout(
            &document,
            &styles,
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
}
