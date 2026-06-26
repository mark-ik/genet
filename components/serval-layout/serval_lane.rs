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
//! ## Implemented vs. stubbed
//!
//! - `FragmentQuery::hit_test` walks the DOM in **paint order** (pre-order
//!   document order, the same order `paint_emit` produces its command stream)
//!   and keeps the **last** containing rect, matching "topmost in paint order
//!   is later in the list." Clip- and scroll-aware (see `hit_test_walk`).
//! - `FragmentQuery::box_model` returns distinct content / padding / border /
//!   margin rects, derived from the taffy border box by the cascaded
//!   border / padding / margin (they collapse to the border box for an
//!   unstyled element).
//! - Anchor + selection-range methods (`fragments_for_anchor`,
//!   `text_range_for_fragment`, `rects_for_selection`) return empty/None: no
//!   anchor index or source-range / line-box tracking is threaded through yet.
//! - `InteractionQuery::affordances_at` / `activation_target` derive from the
//!   DOM at the hit point (links / buttons / form controls / editables /
//!   scrollables), walking hit→root. They cover **block-level** interactive
//!   elements; inline links need inline hit-testing (runs would have to track
//!   their source element), a follow-on. `focus_target` / `selection` return the
//!   host-supplied state from [`ServalLaneView::with_interaction`].
//! - Dynamic pseudo-class restyle (`:hover` / `:focus` changing computed style)
//!   is separate: it needs a state-change snapshot path parallel to the
//!   attribute path (see `StylePlane::set_element_state`).
//!
//! The `SourceNodeId ↔ D::NodeId` round-trip uses `LayoutDom::opaque_id`
//! (forward) + a DOM walk (reverse). The reverse walk is O(n) per `box_model`
//! call, fixable with a reverse-index `FxHashMap<u64, D::NodeId>` cached on the
//! view if a consumer pulls on perf.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::ControlFlow;

use engine_observables_api::{
    Affordance, AffordanceKind, BoxModel, FragmentHit, FragmentQuery, InteractionQuery,
    InteractionState, Point, Rect, Selection, SourceNodeId, SourceRange,
};
use layout_dom_api::LayoutDom;
use paint_list_api::{LayoutPoint, LayoutTransform};

use crate::fragment::FragmentPlane;
use crate::paint_emit::{
    clips_overflow, compute_transform_matrix, conjugate_at, is_fixed, pointer_events_none,
    primary_cv, ScrollOffsets,
};
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
    /// Document (viewport) scroll offset, for viewport-scroll-aware hit-testing:
    /// in-flow content maps through `+offset` (the inverse of paint's `-offset`
    /// document translate); `position: fixed` subtrees counter it (they paint
    /// pinned to the viewport). `(0.0, 0.0)` ⇒ the document is not scrolled. The
    /// host supplies the same offset it feeds the paint viewport.
    viewport_scroll: (f32, f32),
    /// Host-owned focus + selection, surfaced by [`InteractionQuery`]. The host
    /// owns input, so it supplies these; `None` means "nothing focused / no
    /// selection". (Affordances + activation targets are derived from the DOM,
    /// so they need no host state.)
    focused: Option<SourceNodeId>,
    selection: Option<Selection>,
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
            viewport_scroll: (0.0, 0.0),
            focused: None,
            selection: None,
        }
    }

    pub fn with_generation(mut self, gen_id: u64) -> Self {
        self.generation = gen_id;
        self
    }

    /// Supply the host's current focus + selection, answered back by
    /// [`InteractionQuery::focus_target`] / [`InteractionQuery::selection`].
    pub fn with_interaction(
        mut self,
        focused: Option<SourceNodeId>,
        selection: Option<Selection>,
    ) -> Self {
        self.focused = focused;
        self.selection = selection;
        self
    }

    /// Supply the host's full [`InteractionState`] snapshot, so query read-back
    /// (`focus_target` / `selection`) and the dynamic-pseudo-class cascade
    /// (via [`crate::cascade::restyle_for_interaction`], fed the same snapshot)
    /// share one source of truth. The hover / active fields drive the cascade
    /// only; the query surface exposes focus and selection.
    pub fn with_interaction_state(mut self, state: &InteractionState) -> Self {
        self.focused = state.focused;
        self.selection = state.selection;
        self
    }

    /// Supply per-node scroll offsets so hit-testing maps the query point
    /// through scrolled containers (and clips to overflow boxes). Pass the same
    /// map handed to paint.
    pub fn with_scroll_offsets(mut self, offsets: &'a ScrollOffsets<D::NodeId>) -> Self {
        self.scroll_offsets = Some(offsets);
        self
    }

    /// Supply the document (viewport) scroll offset so hit-testing maps the query
    /// point through the scrolled document (and keeps `position: fixed` subtrees
    /// pinned). Pass the same offset handed to the paint viewport.
    pub fn with_viewport_scroll(mut self, scroll: (f32, f32)) -> Self {
        self.viewport_scroll = scroll;
        self
    }

    /// Reverse-lookup a `D::NodeId` for a given `SourceNodeId`.
    /// O(n) over the DOM; acceptable for probe-stage. Cf. module doc.
    /// `pub(crate)` so the `IncrementalLayout` session can serve a hit-test
    /// (`SourceNodeId` → `D::NodeId`) off its retained planes.
    pub(crate) fn find_by_source_id(&self, source_id: SourceNodeId) -> Option<D::NodeId>
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
        // Document (viewport) scroll: paint translates the whole document by
        // `-viewport_scroll` inside the canvas, so the inverse maps the query point
        // by `+viewport_scroll` into content space (the same shape as the per-
        // container `+offset` map in `walk_for_hit`). `position: fixed` subtrees
        // counter it back there (they paint pinned). Zero scroll leaves the point
        // untouched — the unscrolled frame is unchanged.
        let (sx, sy) = self.viewport_scroll;
        let doc_point = Point::new(point.x + sx, point.y + sy);
        let mut hit: Option<FragmentHit<SourceNodeId>> = None;
        walk_for_hit(
            self.dom,
            self.styles,
            self.fragments,
            self.scroll_offsets,
            self.viewport_scroll,
            self.dom.document(),
            Point::new(0.0, 0.0),
            doc_point,
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
        self.focused
    }

    fn selection(&self) -> Option<Selection> {
        self.selection
    }

    fn affordances_at(&self, point: Point) -> Vec<Affordance> {
        let Some(hit) = self.hit_test(point) else { return Vec::new() };
        let Some(start) = self.find_by_source_id(hit.source_node) else {
            return Vec::new();
        };
        // Walk hit -> root; each interactive ancestor contributes an affordance
        // (e.g. a link inside a scroll container surfaces both).
        let mut out = Vec::new();
        let mut cur = Some(start);
        while let Some(id) = cur {
            if let Some(kind) = affordance_kind(self.dom, id) {
                out.push(Affordance {
                    kind,
                    source_node: SourceNodeId(self.dom.opaque_id(id)),
                    label: affordance_label(self.dom, id, kind),
                });
            } else if primary_cv(self.styles, id).as_deref().is_some_and(clips_overflow) {
                out.push(Affordance {
                    kind: AffordanceKind::Scrollable,
                    source_node: SourceNodeId(self.dom.opaque_id(id)),
                    label: None,
                });
            }
            cur = self.dom.parent(id);
        }
        out
    }

    fn activation_target(&self, point: Point) -> Option<SourceNodeId> {
        let hit = self.hit_test(point)?;
        let mut cur = self.find_by_source_id(hit.source_node);
        // Nearest activatable ancestor: a link / button / form control / editable
        // is what a default click acts on (scrollables / hover targets are not).
        while let Some(id) = cur {
            if affordance_kind(self.dom, id).is_some() {
                return Some(SourceNodeId(self.dom.opaque_id(id)));
            }
            cur = self.dom.parent(id);
        }
        None
    }
}

/// Classify an element's input affordance from its tag (and a couple of
/// attributes), or `None` for a non-interactive element. Scrollable containers
/// are detected separately (they are a style property, not a tag).
fn affordance_kind<D: LayoutDom>(dom: &D, id: D::NodeId) -> Option<AffordanceKind> {
    use html5ever::{local_name, ns};
    let no_ns = ns!();
    let name = dom.element_name(id)?;
    let local = &name.local;
    Some(if *local == local_name!("a") {
        // A bare anchor with no href is not a link affordance.
        dom.attribute(id, &no_ns, &local_name!("href"))?;
        AffordanceKind::Link
    } else if *local == local_name!("button") {
        AffordanceKind::Button
    } else if *local == local_name!("textarea") {
        AffordanceKind::Editable
    } else if *local == local_name!("select") {
        AffordanceKind::FormControl
    } else if *local == local_name!("input") {
        match dom.attribute(id, &no_ns, &local_name!("type")) {
            Some("button") | Some("submit") | Some("reset") | Some("image") => {
                AffordanceKind::Button
            },
            Some("checkbox") | Some("radio") | Some("range") | Some("color") | Some("file") => {
                AffordanceKind::FormControl
            },
            Some("hidden") => return None,
            // text / search / email / password / number / … and the default.
            _ => AffordanceKind::Editable,
        }
    } else if dom
        .attribute(id, &no_ns, &local_name!("contenteditable"))
        .is_some_and(|v| v != "false")
    {
        AffordanceKind::Editable
    } else {
        return None;
    })
}

/// A short label for an affordance: a link's `href` (others have none here).
fn affordance_label<D: LayoutDom>(dom: &D, id: D::NodeId, kind: AffordanceKind) -> Option<String> {
    use html5ever::{local_name, ns};
    match kind {
        AffordanceKind::Link => {
            dom.attribute(id, &ns!(), &local_name!("href")).map(str::to_owned)
        },
        _ => None,
    }
}

/// Walk in paint order accumulating origin; if `point` falls in this node's
/// fragment, record the hit (overwriting any earlier hit — "topmost in paint
/// order is later in the walk"). Clip- and scroll-aware, mirroring paint:
///   * an overflow container clips its descendants to its padding box, so a
///     point outside that box skips the subtree (no leak onto elements below a
///     scrolled box);
///   * a scroll container's descendants are queried at `point + offset` (the
///     inverse of paint's `-offset` content translate);
///   * a `position: fixed` node counters the document scroll by `-viewport_scroll`
///     (the inverse of paint's `+viewport_scroll` layer counter), so a pinned box
///     is hit at its unscrolled screen position. The caller has already mapped the
///     incoming point by `+viewport_scroll` for the document at large.
#[allow(clippy::too_many_arguments)]
fn walk_for_hit<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    scroll_offsets: Option<&ScrollOffsets<D::NodeId>>,
    viewport_scroll: (f32, f32),
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

    let cv = primary_cv(styles, id);
    // `position: fixed` attaches to the viewport: paint counters the document
    // scroll around its layer (it paints pinned), so undo the caller's
    // `+viewport_scroll` map here and hit-test the fixed box and its subtree at
    // their unscrolled position. (Common case: a fixed box at document level;
    // nested-in-a-scroller interactions inherit paint's existing stacking
    // approximation, since this walk is DOM-order, not stacking-order.)
    let point = if cv.as_deref().is_some_and(is_fixed) {
        Point::new(point.x - viewport_scroll.0, point.y - viewport_scroll.1)
    } else {
        point
    };

    // CSS transform: a transformed node (and its subtree) is painted through its
    // transform conjugated at the box origin — the exact composition
    // `paint_emit::walk` applies. Hit the inverse: map the incoming point into
    // this node's own pre-transform space, so a hit resolves where paint drew it.
    // Identity (the common case) leaves the point untouched; the inverse
    // telescopes through nesting because each node maps the already-mapped point
    // it receives. A singular transform (a degenerate scale) collapses the
    // subtree, so nothing in it can be hit.
    let node_transform =
        cv.as_deref().map(compute_transform_matrix).unwrap_or_else(LayoutTransform::identity);
    let local = if node_transform == LayoutTransform::identity() {
        point
    } else {
        let eff = conjugate_at((origin.x, origin.y), node_transform);
        match eff.inverse().and_then(|inv| inv.transform_point2d(LayoutPoint::new(point.x, point.y)))
        {
            Some(p) => Point::new(p.x, p.y),
            None => return,
        }
    };

    if let Some(l) = layout {
        let rect = Rect::new(origin, l.size.width, l.size.height);
        // `pointer-events: none` makes the box not a hit target: the point falls
        // through to whatever sits behind it. The walk still descends into children,
        // so a `pointer-events: auto` descendant of a `none` box stays hittable (the
        // computed value already encodes the inheritance).
        if rect.contains(local) && !cv.as_deref().is_some_and(pointer_events_none) {
            *out = Some(FragmentHit {
                fragment: SourceNodeId(dom.opaque_id(id)),
                source_node: SourceNodeId(dom.opaque_id(id)),
                local_point: Point::new(local.x - origin.x, local.y - origin.y),
            });
        }

        // Clip: an overflow container clips its descendants to its padding box.
        // A point outside that box can't hit them — skip the subtree (this is
        // what stops a scrolled box's clicks leaking onto the element below it).
        if cv.as_deref().is_some_and(clips_overflow) {
            let pad = Rect::new(
                Point::new(origin.x + l.border.left, origin.y + l.border.top),
                l.size.width - l.border.left - l.border.right,
                l.size.height - l.border.top - l.border.bottom,
            );
            if !pad.contains(local) {
                return;
            }
        }
    }

    // Scroll: descendants of a scroll container are painted translated by
    // `-offset`, so query them at `point + offset` (in this node's local space).
    let child_point = match scroll_offsets.and_then(|m| m.get(&id)) {
        Some(&(ox, oy)) => Point::new(local.x + ox, local.y + oy),
        None => local,
    };

    for child in dom.dom_children(id) {
        walk_for_hit(
            dom, styles, fragments, scroll_offsets, viewport_scroll, child, origin, child_point,
            out,
        );
    }
}

/// The accumulated CSS-transform translate from the document root down to `target`, in scene
/// px: the sum of each node-on-the-path's `transform` translate component. Paint shifts a
/// transformed box (and its subtree) by this, but the layout fragments do not carry it, so an
/// overlay positioned from fragments (the host focus ring) adds this to land where the box
/// paints — the paint-side complement to `walk_for_hit`'s transform-aware hit-testing.
/// Translate-only (an orrery card translates); a full transform compose is a later refinement.
/// `(0, 0)` for an untransformed path or an unreachable `target`.
pub fn accumulated_translate<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    target: D::NodeId,
) -> (f32, f32)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn walk<D>(
        dom: &D,
        styles: &StylePlane<D::NodeId>,
        id: D::NodeId,
        target: D::NodeId,
        acc: (f32, f32),
    ) -> Option<(f32, f32)>
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        let cv = primary_cv(styles, id);
        let m = cv
            .as_deref()
            .map(compute_transform_matrix)
            .unwrap_or_else(LayoutTransform::identity);
        let acc = (acc.0 + m.m41, acc.1 + m.m42);
        if id == target {
            return Some(acc);
        }
        for child in dom.dom_children(id) {
            if let Some(found) = walk(dom, styles, child, target, acc) {
                return Some(found);
            }
        }
        None
    }
    walk(dom, styles, dom.document(), target, (0.0, 0.0)).unwrap_or((0.0, 0.0))
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

/// Walk the DOM in document order from `id` (whose parent paints at `parent_origin`),
/// computing each node's absolute origin by folding parent-relative taffy locations down
/// the tree, and calling `visit(node, origin)` for each. When `scroll` is `Some`, a node
/// that is a scroll container shifts its descendants' origin by its retained offset, so
/// `visit` receives the **painted** origin (where the node lands after ancestors' scroll);
/// when `None`, the unscrolled layout origin. `visit` returns [`ControlFlow`] so a
/// single-target caller can stop early. The one origin-accumulation core for
/// [`absolute_origin`], [`accumulate_origins`], and [`accumulate_painted_origins`].
fn walk_origins<D, F>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    parent_origin: Point,
    scroll: Option<&ScrollOffsets<D::NodeId>>,
    visit: &mut F,
) -> ControlFlow<()>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
    F: FnMut(D::NodeId, Point) -> ControlFlow<()>,
{
    let origin = match fragments.rect_of(id) {
        Some(l) => Point::new(parent_origin.x + l.location.x, parent_origin.y + l.location.y),
        None => parent_origin,
    };
    visit(id, origin)?;
    // A scroll container paints its descendants shifted by `-offset` (the content scrolls
    // under the container's clip); a non-scrolled node passes its origin straight through.
    let child_origin = match scroll.and_then(|s| s.get(&id)) {
        Some(&(sx, sy)) => Point::new(origin.x - sx, origin.y - sy),
        None => origin,
    };
    for child in dom.dom_children(id) {
        walk_origins(dom, fragments, child, child_origin, scroll, visit)?;
    }
    ControlFlow::Continue(())
}

/// The absolute (layout-space, unscrolled) origin of `target`, or `None` if not reachable.
/// O(n) worst case (stops early once `target` is reached). Public so hosts and overlay
/// producers (scrollbar thumbs, focus rings, anchored popups) can read an element's
/// document-space origin off the fragment plane instead of re-rolling the parent-relative
/// accumulation. For many nodes at once use [`accumulate_origins`].
pub fn absolute_origin<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    target: D::NodeId,
) -> Option<Point>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut found = None;
    let _ = walk_origins(dom, fragments, dom.document(), Point::new(0.0, 0.0), None, &mut |id, o| {
        if id == target {
            found = Some(o);
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    found
}

/// The absolute (layout-space, unscrolled) origin of **every** laid-out node, keyed by node
/// — one O(n) pass down the tree. The batch form of [`absolute_origin`], for a host or
/// serval-render that needs many nodes' origins at once (a11y row bounds, scrollbar overlays)
/// rather than re-rolling the accumulation per consumer.
pub fn accumulate_origins<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
) -> HashMap<D::NodeId, Point>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut out = HashMap::new();
    let _ = walk_origins(dom, fragments, dom.document(), Point::new(0.0, 0.0), None, &mut |id, o| {
        out.insert(id, o);
        ControlFlow::Continue(())
    });
    out
}

/// The **painted** origin of every laid-out node, keyed by node: like [`accumulate_origins`]
/// but each scroll container's retained `scroll` offset shifts its descendants, so an entry
/// is where the node actually paints after its ancestors' nested scroll. The scroll-aware
/// answer serval otherwise has no public form of, for overlays / selection handles / IME
/// anchoring / a11y bounds that must track scrolled content. Pass [`IncrementalLayout::
/// element_scroll`](crate::IncrementalLayout::element_scroll) as `scroll`.
pub fn accumulate_painted_origins<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    scroll: &ScrollOffsets<D::NodeId>,
) -> HashMap<D::NodeId, Point>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut out = HashMap::new();
    let _ = walk_origins(dom, fragments, dom.document(), Point::new(0.0, 0.0), Some(scroll), &mut |id, o| {
        out.insert(id, o);
        ControlFlow::Continue(())
    });
    out
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

    /// The batch `accumulate_origins` agrees with the single-target `absolute_origin` and
    /// covers every laid-out node — the one O(n) pass a many-node consumer (a11y, scrollbars)
    /// reads instead of re-rolling the accumulation.
    #[test]
    fn accumulate_origins_matches_per_node_walk() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);

        let map = accumulate_origins(&document, &fragments);
        let p = find_element(NodeRef::document(&document), local_name!("p")).expect("<p>");
        assert_eq!(
            map.get(&p.id()).copied(),
            absolute_origin(&document, &fragments, p.id()),
            "the batch map agrees with the single-target walk for <p>",
        );
        assert!(map.len() >= 3, "html / body / p each get an entry");
    }

    /// `accumulate_painted_origins` shifts a scroll container's descendants by `-offset` (the
    /// painted position after nested scroll) while leaving the container itself put — the
    /// scroll-aware origin a11y / overlay / IME anchoring needs.
    #[test]
    fn painted_origins_subtract_an_ancestor_scroll() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);

        let body = find_element(NodeRef::document(&document), local_name!("body")).expect("<body>");
        let p = find_element(NodeRef::document(&document), local_name!("p")).expect("<p>");

        let unscrolled = accumulate_origins(&document, &fragments);
        let mut scroll = ScrollOffsets::<StaticNodeId>::default();
        scroll.insert(body.id(), (0.0, 40.0)); // body scrolled 40px down
        let painted = accumulate_painted_origins(&document, &fragments, &scroll);

        assert_eq!(
            painted.get(&body.id()),
            unscrolled.get(&body.id()),
            "the scrolled container itself is the scrollport, so it is not shifted",
        );
        let up = unscrolled.get(&p.id()).expect("<p> origin");
        let pp = painted.get(&p.id()).expect("<p> painted origin");
        assert!((pp.y - (up.y - 40.0)).abs() < 0.5, "<p> paints 40px up under the scrolled body");
        assert!((pp.x - up.x).abs() < 0.5, "x unchanged (no horizontal scroll)");
    }

    /// G1.2 transform-aware hit-testing: a `<p>` translated 120px right is
    /// painted at `origin + 120`, so a point there must resolve to it. The
    /// DOM-driven walk composes box *locations*; without folding the CSS
    /// transform it tests the un-transformed rect, so the painted point lands on
    /// `<body>` behind the `<p>` instead. The fix maps the point through the
    /// node's transform inverse (mirroring paint), so hit matches where it drew.
    #[test]
    fn hit_test_resolves_a_point_inside_a_translated_subtree() {
        let document =
            StaticDocument::parse(r#"<html><body><p class="moved">x</p></body></html>"#);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        crate::cascade::run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "p { display: block; width: 200px; height: 50px; margin: 0; padding: 0; border: 0; }",
                ".moved { transform: translate(120px, 0px); }",
            ],
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        let p = find_element(NodeRef::document(&document), local_name!("p")).expect("<p> exists");
        let p_origin = absolute_origin(&document, &fragments, p.id()).expect("<p> origin");
        let p_layout = fragments.rect_of(p.id()).expect("<p> fragment");
        // The painted center: the un-transformed center shifted by the 120px
        // translate. With width 200, this x (origin + 120 + 100) lies *outside*
        // the un-transformed rect [origin, origin + 200], so a pre-fix walk misses.
        let point = Point::new(
            p_origin.x + 120.0 + p_layout.size.width * 0.5,
            p_origin.y + p_layout.size.height * 0.5,
        );

        let hit = view.hit_test(point).expect("hit something at the painted position");
        let expected = document.opaque_id(p.id());
        assert_eq!(
            hit.source_node.0, expected,
            "a point at the translated <p>'s painted position must resolve to <p> \
             (opaque_id {expected}), got opaque_id {}",
            hit.source_node.0,
        );
    }

    /// G1.2, the conjugation case a translate can't catch: `scale(2)` about the
    /// box origin grows the 200×50 `<p>` to 400×100, so a point at `(origin +
    /// 250, origin + 25)` is inside the *painted* box but outside the
    /// un-transformed one. It resolves to `<p>` only if the hit maps through the
    /// transform conjugated at the box origin (not a naive translate).
    #[test]
    fn hit_test_resolves_a_point_inside_a_scaled_subtree() {
        let document =
            StaticDocument::parse(r#"<html><body><p class="big">x</p></body></html>"#);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        crate::cascade::run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "p { display: block; width: 200px; height: 50px; margin: 0; padding: 0; border: 0; }",
                ".big { transform: scale(2); }",
            ],
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        let p = find_element(NodeRef::document(&document), local_name!("p")).expect("<p> exists");
        let p_origin = absolute_origin(&document, &fragments, p.id()).expect("<p> origin");
        // x = origin + 250 is outside the un-scaled [origin, origin+200] but
        // inside the painted [origin, origin+400]; y = origin + 25 is inside both.
        let point = Point::new(p_origin.x + 250.0, p_origin.y + 25.0);

        let hit = view.hit_test(point).expect("hit something at the painted position");
        let expected = document.opaque_id(p.id());
        assert_eq!(
            hit.source_node.0, expected,
            "a point in the scaled <p>'s painted box must resolve to <p> \
             (opaque_id {expected}), got opaque_id {}",
            hit.source_node.0,
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

    /// A hit on a block `::before`'s region routes to the originating element
    /// (browser-faithful). The pseudo box has no DOM node, so hit-testing never
    /// visits it — but it is laid out *inside* the element's enclosing box, so the
    /// point falls in the element's fragment and resolves to it. (Pseudo
    /// follow-ups §5 slice 4: routing is structural, no extra plumbing.)
    #[test]
    fn hit_on_block_before_pseudo_routes_to_element() {
        let document = StaticDocument::parse("<html><body><p>hi</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        crate::cascade::run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "html, body, p { display: block; margin: 0; }",
                "p::before { content: \"X\"; display: block; height: 20px; }",
            ],
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let view = ServalLaneView::new(&document, &styles, &fragments);

        let p = find_element(NodeRef::document(&document), local_name!("p")).expect("<p>");
        let p_origin = absolute_origin(&document, &fragments, p.id()).expect("<p> origin");
        // A point 10px down — inside the ::before band (top 20px of <p>).
        let point = Point::new(p_origin.x + 5.0, p_origin.y + 10.0);

        let hit = view.hit_test(point).expect("hit something");
        assert_eq!(
            hit.source_node.0,
            document.opaque_id(p.id()),
            "a hit on the block ::before region routes to <p>, not a phantom pseudo node"
        );
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

    /// `InteractionQuery` over a block link: affordances + activation derive from
    /// the DOM (the link is the activation target and carries a Link affordance
    /// labelled with its href), and focus reflects the host-supplied state.
    /// (Inline links need inline hit-testing, a follow-on.)
    #[test]
    fn interaction_query_derives_affordances_and_reflects_host_state() {
        let document =
            StaticDocument::parse("<html><body><a href=\"u\">link</a></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        crate::cascade::run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["a { display: block; width: 200px; height: 50px; }"],
            None,
        );
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

        let a = find_element(NodeRef::document(&document), local_name!("a")).expect("<a> exists");
        let a_src = SourceNodeId(document.opaque_id(a.id()));
        let origin = absolute_origin(&document, &fragments, a.id()).expect("<a> origin");
        let rect = fragments.rect_of(a.id()).expect("<a> fragment");
        let point =
            Point::new(origin.x + rect.size.width * 0.5, origin.y + rect.size.height * 0.5);

        let affs = view.affordances_at(point);
        assert!(
            affs.iter().any(|aff| aff.kind == AffordanceKind::Link
                && aff.source_node == a_src
                && aff.label.as_deref() == Some("u")),
            "link affordance with href label at the <a>: {affs:?}"
        );
        assert_eq!(view.activation_target(point), Some(a_src), "activation = the link");

        // Host focus flows through `with_interaction`.
        let focused = view.with_interaction(Some(a_src), None);
        assert_eq!(focused.focus_target(), Some(a_src));

        // The same focus, supplied as a full `InteractionState` snapshot — the
        // identical source the cascade's `restyle_for_interaction` consumes, so
        // read-back and dynamic-pseudo-class restyle share one snapshot.
        let snapshot = InteractionState { focused: Some(a_src), ..Default::default() };
        let via_state =
            ServalLaneView::new(&document, &styles, &fragments).with_interaction_state(&snapshot);
        assert_eq!(via_state.focus_target(), Some(a_src));
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

    /// A4 — in-flow hit-testing maps through the document (viewport) scroll: the
    /// same screen point resolves to whatever content scrolled under it, and the
    /// reported local point reflects the scrolled-in offset (mirroring paint's
    /// `-viewport_scroll` document translate).
    #[test]
    fn hit_test_in_flow_maps_through_viewport_scroll() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse(
            "<html><body>\
                <div class=\"top\">t</div>\
                <div class=\"mid\">m</div>\
                <div class=\"bot\">b</div>\
            </body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(200.0, 600.0),
            &[
                "html, body { display: block; margin: 0; padding: 0; }",
                "div { display: block; margin: 0; padding: 0; border: 0; height: 100px; }",
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
        assert_eq!(divs.len(), 3, "top, mid, bot");
        let id_of = |n: &NodeRef<StaticDocument>| SourceNodeId(document.opaque_id(n.id()));
        let (mid_id, bot_id) = (id_of(&divs[1]), id_of(&divs[2]));

        // Unscrolled: a screen point at y=150 hits `mid` (layout y 100..200).
        let still = ServalLaneView::new(&document, &styles, &fragments);
        assert_eq!(
            still.hit_test(Point::new(5.0, 150.0)).expect("hit").source_node,
            mid_id,
            "unscrolled, y=150 is in `mid`",
        );

        // Scrolled down 100px: the document shifts up by 100, so the same screen
        // point now maps to content-y 250 and hits `bot` (layout y 200..300).
        let scrolled = ServalLaneView::new(&document, &styles, &fragments)
            .with_viewport_scroll((0.0, 100.0));
        let hit = scrolled.hit_test(Point::new(5.0, 150.0)).expect("hit under scroll");
        assert_eq!(hit.source_node, bot_id, "scrolled, the same point is over `bot`");
        assert!(
            (hit.local_point.y - 50.0).abs() < 0.1,
            "local point maps through the viewport scroll: {}",
            hit.local_point.y,
        );
    }

    /// A4 — a `position: fixed` box is hit at its unscrolled screen position: its
    /// layer counters the document scroll in paint, so the hit walk counters too.
    /// Below the fixed box, the scrolled-in in-flow content is hit (so in-flow
    /// moved while the fixed box did not). The fixed div is last in DOM so it wins
    /// overlapping points, mirroring its on-top paint.
    #[test]
    fn hit_test_keeps_fixed_pinned_under_viewport_scroll() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse(
            "<html><body>\
                <div class=\"tall\">t</div>\
                <div class=\"fixed\">f</div>\
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
                ".tall { height: 2000px; }",
                ".fixed { position: fixed; top: 0; left: 0; width: 100px; height: 50px; }",
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
        let id_of = |n: &NodeRef<StaticDocument>| SourceNodeId(document.opaque_id(n.id()));
        let (tall_id, fixed_id) = (id_of(&divs[0]), id_of(&divs[1]));

        // Unscrolled, a point in the fixed box (top-left) hits it.
        let still = ServalLaneView::new(&document, &styles, &fragments);
        assert_eq!(
            still.hit_test(Point::new(5.0, 5.0)).expect("hit").source_node,
            fixed_id,
            "the fixed box is hit at its screen position",
        );

        // Scrolled down 300px: the fixed box stays pinned (hit-tested unscrolled),
        // so the same screen point still hits it, not the scrolled-in `tall`.
        let scrolled = ServalLaneView::new(&document, &styles, &fragments)
            .with_viewport_scroll((0.0, 300.0));
        assert_eq!(
            scrolled.hit_test(Point::new(5.0, 5.0)).expect("hit under scroll").source_node,
            fixed_id,
            "the fixed box stays pinned under document scroll",
        );
        // Below the fixed box (y=100, outside its 50px height), the point maps
        // through the scroll into `tall`'s scrolled-in content.
        assert_eq!(
            scrolled.hit_test(Point::new(5.0, 100.0)).expect("hit below fixed").source_node,
            tall_id,
            "below the fixed box, the scrolled-in in-flow content is hit",
        );
    }
}
