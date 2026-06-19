/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stateful incremental layout session — the live wiring of fine-grained
//! restyle into the layout loop.
//!
//! [`IncrementalLayout`] persists the `StylePlane` (cascaded
//! `ElementData`) and `FragmentPlane` across mutations, so it can drive
//! Stylo's *incremental* restyle ([`restyle_with_snapshots`]) instead of
//! re-cascading from scratch — and then **skip layout entirely** when the
//! restyle's `RestyleDamage` is paint-only (e.g. a `color` swap).
//!
//! This is what the scripted tier's relayout-on-mutation routes through:
//! an attribute-only mutation batch restyles incrementally and re-lays-out
//! only if box geometry changed; a structural batch (insert / remove /
//! `innerHTML`) falls back to a correct full cascade + layout (those go
//! through the relayout-scope path, not the attribute/state invalidator).
//!
//! Cf. `docs/2026-05-25_fine_grained_restyle_plan.md`.

use std::hash::Hash;

use engine_observables_api::{FragmentQuery, Point};
use layout_dom_api::{DomMutation, LayoutDom};
use paint_list_api::DeviceIntSize;
use style::selector_parser::RestyleDamage;
use style::stylist::Stylist;

use crate::box_tree::BoxTree;
use crate::cascade::{
    build_stylist, restyle_structural, restyle_with_snapshots, run_cascade_with_stylist,
};
use crate::fragment::FragmentPlane;
use crate::image_decode::{BackgroundImagePlane, ImagePlane};
use crate::invalidate::{classify, coalesce};
use crate::paint_emit::{emit_paint_list_scrolled, ScrollOffsets, ServalPaintList};
use crate::serval_lane::ServalLaneView;
use crate::style::StylePlane;
use crate::subtree::SubtreeView;
use crate::text_measure::TextMeasureCtx;
use crate::viewport::{document_scroll_range, ScrollKey, Viewport};

/// What [`IncrementalLayout::apply`] did for a mutation batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Applied {
    /// No mutations — nothing changed.
    Unchanged,
    /// Attribute-only batch, restyled incrementally, **paint-only** —
    /// layout was skipped (the prior `FragmentPlane` still holds).
    RepaintOnly,
    /// Attribute-only batch, restyled incrementally, and re-laid-out
    /// (box geometry changed).
    Restyled,
    /// Structural batch, laid out **incrementally** — each affected
    /// subtree re-laid-out and spliced into the prior fragments at its
    /// real position (outer size unchanged).
    Spliced,
    /// Full cascade + layout — the conservative fallback (a spliced
    /// subtree's outer size changed, so ancestors would reflow, or a
    /// root wasn't previously laid out).
    FullRecompute,
}

/// A persistent cascade + layout session over one `LayoutDom`.
pub struct IncrementalLayout<Id: Copy + Eq + Hash> {
    styles: StylePlane<Id>,
    /// The **persistent** Stylist (device + UA/author sheets + rule tree), built
    /// once in [`new`](Self::new) and reused for every pass. It must outlive the
    /// `styles` plane and never be rebuilt mid-session: `ElementData` in `styles`
    /// holds `StrongRuleNode`s into this Stylist's rule tree, and the incremental
    /// replacement path (`RESTYLE_STYLE_ATTRIBUTE`) reuses them — a rule node from
    /// a dropped tree is a use-after-free. This is the half that makes the cheap
    /// per-frame inline-style restyle sound (the other half, a stable
    /// `SharedRwLock`, already lives on `StylePlane`).
    stylist: Stylist,
    /// The stylesheet set `stylist` was built from. The session's stylesheets are
    /// FIXED at construction (the persistent rule tree can't be safely rebuilt
    /// mid-session — old rule nodes would dangle); [`apply`](Self::apply)
    /// debug-asserts the caller keeps passing the same set. Hot-reload (rebuild
    /// the Stylist + force a full re-match that frame, dropping old nodes while
    /// the old tree is still alive) is a documented follow-up.
    sheets: Vec<String>,
    fragments: FragmentPlane<Id>,
    /// The box tree + text-measure context from the most recent **full** layout
    /// (`new` / a relayout). Retained so a host can emit a glyph-bearing paint
    /// list ([`emit_paint_list`](Self::emit_paint_list)) on the cheap
    /// `RepaintOnly` path — a transform-only frame keeps box geometry, so these
    /// stay valid without a relayout.
    built: BoxTree<Id>,
    text_ctx: TextMeasureCtx,
    /// Decoded `<img>` images (data: URIs) keyed by node, rebuilt at every full
    /// layout alongside `built` / `text_ctx`, so the cheap `RepaintOnly` emit paints
    /// `<img>` content (e.g. the chrome card favicons) without re-decoding per frame.
    /// Empty for a document with no `<img>`; remote URLs are skipped (data: only — the
    /// session carries no host loader), which is exactly the chrome's data-URI favicons.
    images: ImagePlane<Id>,
    /// Whether `built` / `text_ctx` still match `fragments`. Set by every full
    /// layout; cleared by a structural splice (which updates `fragments` but not
    /// the box-tree side-table). [`emit_paint_list`](Self::emit_paint_list)
    /// requires it.
    paint_side_valid: bool,
    width: f32,
    height: f32,
    /// The document's [`Viewport`] (size + propagated overflow + scroll) — rule 1's
    /// first-class per-document object, owned by the session because the session
    /// *is* the document (one viewport per content card / iframe / page). Its
    /// overflow + size are recomputed on every relayout (the scroll preserved and
    /// re-clamped); the host drives the scroll through [`set_viewport_scroll`](Self::set_viewport_scroll)
    /// / [`scroll_by`](Self::scroll_by), and [`emit_paint_list`](Self::emit_paint_list)
    /// paints at it.
    viewport: Viewport,
    /// Per-element scroll offsets for nested `overflow: scroll/auto` containers,
    /// retained across frames (parallel to `viewport.scroll` for the document). The
    /// host drives these through [`scroll_at`](Self::scroll_at) (the wheel default
    /// action); [`hit_test`](Self::hit_test) and [`emit_paint_list`](Self::emit_paint_list)
    /// merge them with the caller's own offsets, so a content document's inner
    /// scrollers move while a host's explicit offsets (meerkat's panes) still apply.
    /// Empty until something scrolls, so existing callers are unchanged.
    element_scroll: ScrollOffsets<Id>,
    /// Aggregate `RestyleDamage` from the most recent attribute-only
    /// [`apply`](Self::apply). Lets callers/tests confirm which paint-tier bits
    /// a batch produced (e.g. a transform-only motion frame registers
    /// `RECALCULATE_OVERFLOW` without `RELAYOUT`). `empty()` before any restyle.
    last_damage: RestyleDamage,
}

impl<Id: Copy + Eq + Hash + Send + Sync + 'static> IncrementalLayout<Id> {
    /// Initial full cascade + layout over `dom`. Builds the persistent Stylist
    /// (see [`stylist`](Self::stylist)) and runs the first cascade over it, so the
    /// rule tree the incremental passes later reuse is the one this populates.
    pub fn new<D>(dom: &D, stylesheets: &[&str], width: f32, height: f32) -> Self
    where
        D: LayoutDom<NodeId = Id>,
    {
        let mut styles = StylePlane::new();
        // Build the persistent Stylist under the plane's stable lock, so the
        // sheets here, the inline-style blocks parsed each pass, and the cascade
        // guards all share one `SharedRwLock` (Stylo's `same_lock_as`).
        let lock = styles.shared_lock().clone();
        let quirks = crate::adapter_stylo::selectors_quirks_mode(dom.quirks_mode());
        let stylist =
            build_stylist(euclid::Size2D::new(width, height), stylesheets, None, &lock, quirks);
        run_cascade_with_stylist(dom, &mut styles, &stylist);
        let mut text_ctx = TextMeasureCtx::new();
        let (fragments, built) = full_layout(dom, &styles, width, height, &mut text_ctx);
        // The document viewport: propagated overflow over the first cascade, scroll
        // at the origin. Recomputed on every relayout (overflow + size), the host's
        // scroll preserved and re-clamped (see `recompute_viewport`).
        let viewport = Viewport::for_document(dom, &styles, DeviceIntSize::new(width as i32, height as i32));
        Self {
            styles,
            stylist,
            sheets: stylesheets.iter().map(|s| s.to_string()).collect(),
            fragments,
            built,
            text_ctx,
            images: ImagePlane::decode_from_dom(dom),
            paint_side_valid: true,
            width,
            height,
            viewport,
            element_scroll: ScrollOffsets::default(),
            last_damage: RestyleDamage::empty(),
        }
    }

    /// The current per-node fragment plane.
    pub fn fragments(&self) -> &FragmentPlane<Id> {
        &self.fragments
    }

    /// The current cascaded style plane — the other half (with [`fragments`](Self::fragments))
    /// a `ServalLaneView` hit-test reads, so a host can serve point queries off the
    /// session's retained layout instead of re-cascading.
    pub fn styles(&self) -> &StylePlane<Id> {
        &self.styles
    }

    /// The topmost (paint-order) DOM node containing scene point `(x, y)`, served
    /// from the session's retained planes through the `engine_observables_api`
    /// query surface — no re-cascade. The session companion to
    /// `LaidOutDocument::hit_test` / the stateless `hit_test_node`, so a host routes
    /// click and region hit-tests through the same session it renders. Clip- and
    /// scroll-aware via `scroll`, and document-scroll-aware via the session's
    /// viewport (in-flow content maps through the offset, `position: fixed` stays
    /// pinned — the hit mirror of [`emit_paint_list`](Self::emit_paint_list)).
    /// `None` if the point falls outside every fragment.
    pub fn hit_test<D>(&self, dom: &D, x: f32, y: f32, scroll: &ScrollOffsets<Id>) -> Option<Id>
    where
        D: LayoutDom<NodeId = Id>,
    {
        let merged = self.merged_scroll(scroll);
        let view = ServalLaneView::new(dom, &self.styles, &self.fragments)
            .with_scroll_offsets(&merged)
            .with_viewport_scroll(self.viewport.scroll);
        let hit = view.hit_test(Point::new(x, y))?;
        let node = view.find_by_source_id(hit.source_node)?;
        // Inline refinement: a `display:inline` element (`<a>`, `<span>`, …)
        // establishes no box, so the block walk above can only resolve its
        // containing inline-formatting leaf. Descend into that leaf's cached text to
        // recover the inline element under the point — the `elementFromPoint`
        // granularity links and inline interactivity need. Over the leaf's
        // inter-run / empty space this yields `None` and the block leaf stands.
        self.inline_hit_at(node, hit.local_point).or(Some(node))
    }

    /// Resolve a point inside inline-formatting leaf `node` to the inline element
    /// under it (the standards [`elementFromPoint`] descent), or `None` when `node`
    /// is not an inline-formatting leaf or the point misses every run. `local` is the
    /// point relative to `node`'s border-box origin, as [`hit_test`](Self::hit_test)'s
    /// `FragmentHit::local_point` reports it; inline layout is content-box relative,
    /// so border + padding come off first.
    fn inline_hit_at(&self, node: Id, local: Point) -> Option<Id> {
        let taffy_id = self.built.node_map.get(&node)?;
        let layout = self.text_ctx.layouts.get(taffy_id)?;
        let sources = self.built.inline_sources(node)?;
        let frame = self.fragments.rect_of(node)?;
        let cx = local.x - (frame.border.left + frame.padding.left);
        let cy = local.y - (frame.border.top + frame.padding.top);
        let el = crate::inline_hit::inline_source_at(layout, sources, cx, cy)?;
        // `pointer-events: none` on the resolved inline element makes it fall through
        // to the block leaf (already resolved per its own pointer-events). A nested
        // `auto` descendant resolves to itself (innermost wins), so a click on it
        // still hits even inside a `none` ancestor.
        if crate::paint_emit::primary_cv(&self.styles, el)
            .as_deref()
            .is_some_and(crate::paint_emit::pointer_events_none)
        {
            return None;
        }
        Some(el)
    }

    /// The document [`Viewport`] (size + propagated overflow + current scroll).
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// The current document (viewport) scroll offset in device px.
    pub fn viewport_scroll(&self) -> (f32, f32) {
        self.viewport.scroll
    }

    /// The document's maximum scroll offset ([`document_scroll_range`]) — the
    /// extent of its scrollable-overflow region beyond the viewport (rule 4).
    pub fn scroll_range<D>(&self, dom: &D) -> (f32, f32)
    where
        D: LayoutDom<NodeId = Id>,
    {
        document_scroll_range(dom, &self.styles, &self.fragments, self.viewport.size)
    }

    /// Set the document scroll to `scroll` (device px), clamped to the axes the
    /// viewport actually scrolls (propagated overflow — `overflow: hidden` on the
    /// root pins that axis at 0) and to `[0, `[`scroll_range`](Self::scroll_range)`]`.
    /// The host calls this from its wheel / keyboard default action; the next
    /// [`emit_paint_list`](Self::emit_paint_list) paints at the new offset.
    pub fn set_viewport_scroll<D>(&mut self, dom: &D, scroll: (f32, f32))
    where
        D: LayoutDom<NodeId = Id>,
    {
        let range = document_scroll_range(dom, &self.styles, &self.fragments, self.viewport.size);
        self.viewport.scroll = self.viewport.clamp_scroll(scroll, range);
    }

    /// Scroll the document by `(dx, dy)` from its current offset (clamped as in
    /// [`set_viewport_scroll`](Self::set_viewport_scroll)), returning the new
    /// offset. The convenient form for a wheel delta.
    pub fn scroll_by<D>(&mut self, dom: &D, dx: f32, dy: f32) -> (f32, f32)
    where
        D: LayoutDom<NodeId = Id>,
    {
        let target = (self.viewport.scroll.0 + dx, self.viewport.scroll.1 + dy);
        self.set_viewport_scroll(dom, target);
        self.viewport.scroll
    }

    /// Apply a keyboard scroll default action ([`ScrollKey`], scope doc rule 5) to
    /// the document viewport: an arrow steps a line, `PageUp`/`PageDown` step a
    /// viewport (less one line of overlap), `Home`/`End` jump to the top/bottom of
    /// the scroll range — all clamped (so a non-scrollable axis or an edge is a
    /// no-op). Returns whether the offset moved. The shared half of rule 5; the host
    /// maps its key event to a `ScrollKey` and gates on "focus not in an editable".
    pub fn scroll_for_key<D>(&mut self, dom: &D, key: ScrollKey) -> bool
    where
        D: LayoutDom<NodeId = Id>,
    {
        /// Arrow-key step in device px (a few lines).
        const LINE: f32 = 40.0;
        // A page keeps ~one line of the previous viewport visible (reading continuity).
        let page = (self.viewport.size.height as f32 - LINE).max(LINE);
        let before = self.viewport.scroll;
        match key {
            ScrollKey::Up => self.scroll_by(dom, 0.0, -LINE),
            ScrollKey::Down => self.scroll_by(dom, 0.0, LINE),
            ScrollKey::Left => self.scroll_by(dom, -LINE, 0.0),
            ScrollKey::Right => self.scroll_by(dom, LINE, 0.0),
            ScrollKey::PageUp => self.scroll_by(dom, 0.0, -page),
            ScrollKey::PageDown => self.scroll_by(dom, 0.0, page),
            ScrollKey::Home => {
                let x = self.viewport.scroll.0;
                self.set_viewport_scroll(dom, (x, 0.0));
                self.viewport.scroll
            },
            ScrollKey::End => {
                let x = self.viewport.scroll.0;
                let range =
                    document_scroll_range(dom, &self.styles, &self.fragments, self.viewport.size);
                self.set_viewport_scroll(dom, (x, range.1));
                self.viewport.scroll
            },
        };
        self.viewport.scroll != before
    }

    /// Scroll the document so `node`'s top aligns with the viewport top (block-start
    /// `scroll-into-view`), clamped to the scroll range — the shared mechanism behind
    /// anchor-fragment navigation (`url#id` / in-page `#id` links) and focus-into-view
    /// (scope doc rule 5). Returns whether the offset moved; a no-op if `node` has no
    /// fragment.
    ///
    /// Block-start only, so an anchored target lands *under* a fixed header. The spec
    /// offset for that (`scroll-padding` on the viewport, `scroll-margin` on the
    /// target) is **deferred, blocked on stylo**: serval's stylo build does not
    /// compile the CSS Scroll Snap `scroll-padding` / `scroll-margin` longhands (they
    /// are absent from the `Position` / `Padding` / `Margin` / `Box` computed
    /// structs), so there is no value to read. Bare block-start is the spec *default*
    /// regardless (a page with no `scroll-padding` behaves identically). Revisit when
    /// stylo gains the properties.
    pub fn scroll_to_element<D>(&mut self, dom: &D, node: Id) -> bool
    where
        D: LayoutDom<NodeId = Id>,
    {
        let Some(origin) = crate::serval_lane::absolute_origin(dom, &self.fragments, node) else {
            return false;
        };
        let before = self.viewport.scroll;
        self.set_viewport_scroll(dom, (before.0, origin.y));
        self.viewport.scroll != before
    }

    /// Scroll the document the **minimum** needed to bring `node` fully into the
    /// viewport — the `scroll-into-view` "nearest" alignment focus uses (Tab /
    /// autofocus), distinct from [`scroll_to_element`](Self::scroll_to_element)'s
    /// always-top "start" alignment (anchor navigation). A node already fully visible
    /// is a no-op (focus does not jump the page); one off the top brings its top edge
    /// to the viewport top; one off the bottom brings its bottom edge to the viewport
    /// bottom; one larger than the viewport aligns its start (top-/left-edge), so the
    /// element's beginning is visible. Per axis and clamped to the scroll range (a
    /// non-scrolling axis stays put). Returns whether the offset moved; a no-op if
    /// `node` has no fragment.
    ///
    /// Document-viewport scope, like `scroll_to_element`: bringing `node` into view
    /// within an intervening nested scroll container (and then the viewport) is the
    /// recursive `scroll-into-view` refinement, a follow-on on
    /// [`element_scroll`](Self::element_scroll).
    pub fn scroll_element_into_view<D>(&mut self, dom: &D, node: Id) -> bool
    where
        D: LayoutDom<NodeId = Id>,
    {
        let Some(origin) = crate::serval_lane::absolute_origin(dom, &self.fragments, node) else {
            return false;
        };
        let Some(rect) = self.fragments.rect_of(node) else {
            return false;
        };
        let before = self.viewport.scroll;
        let vw = self.viewport.size.width as f32;
        let vh = self.viewport.size.height as f32;
        let (sx, sy) = self.viewport.scroll;
        let (el_left, el_top) = (origin.x, origin.y);
        let (el_right, el_bottom) = (origin.x + rect.size.width, origin.y + rect.size.height);
        // "nearest" per axis: align the off-edge to the matching viewport edge, but
        // never push the element's start edge off (an element taller/wider than the
        // viewport aligns its start). An element already within the window holds.
        let new_y = if el_top < sy {
            el_top
        } else if el_bottom > sy + vh {
            (el_bottom - vh).min(el_top)
        } else {
            sy
        };
        let new_x = if el_left < sx {
            el_left
        } else if el_right > sx + vw {
            (el_right - vw).min(el_left)
        } else {
            sx
        };
        self.set_viewport_scroll(dom, (new_x, new_y));
        self.viewport.scroll != before
    }

    /// Scroll to the element whose `id` attribute is `id` (anchor-fragment
    /// navigation: `url#id` and in-page `#id` links), via
    /// [`scroll_to_element`](Self::scroll_to_element). Returns whether the offset
    /// moved; a no-op if no element has that id.
    pub fn scroll_to_id<D>(&mut self, dom: &D, id: &str) -> bool
    where
        D: LayoutDom<NodeId = Id>,
    {
        match element_by_id(dom, id) {
            Some(node) => self.scroll_to_element(dom, node),
            None => false,
        }
    }

    /// The in-page anchor fragment (`#id` → `"id"`) of a link under scene point
    /// `(x, y)`, or `None`. Hit-tests the point (document- and element-scroll aware)
    /// and walks hit → root for the nearest `<a href="#...">`. The host feeds a click
    /// position to this and, on `Some`, calls [`scroll_to_id`](Self::scroll_to_id) —
    /// in-page link navigation (scope doc rule 5).
    pub fn link_fragment_at<D>(
        &self,
        dom: &D,
        x: f32,
        y: f32,
        scroll: &ScrollOffsets<Id>,
    ) -> Option<String>
    where
        D: LayoutDom<NodeId = Id>,
    {
        let mut cur = self.hit_test(dom, x, y, scroll);
        while let Some(node) = cur {
            if let Some(fragment) = anchor_fragment(dom, node) {
                return Some(fragment);
            }
            cur = dom.parent(node);
        }
        None
    }

    /// The full `href` of the `<a>` link under scene point `(x, y)`, or `None`. Like
    /// [`link_fragment_at`](Self::link_fragment_at) but returns the whole href (a
    /// cross-document URL, a relative path, or an in-page `#fragment`), so the host can
    /// resolve and load a navigation. The host distinguishes an in-page `#…` href
    /// (scroll) from a navigable one (load) on the returned string.
    pub fn link_href_at<D>(
        &self,
        dom: &D,
        x: f32,
        y: f32,
        scroll: &ScrollOffsets<Id>,
    ) -> Option<String>
    where
        D: LayoutDom<NodeId = Id>,
    {
        let mut cur = self.hit_test(dom, x, y, scroll);
        while let Some(node) = cur {
            if let Some(href) = anchor_href(dom, node) {
                return Some(href);
            }
            cur = dom.parent(node);
        }
        None
    }

    /// Merge the session's retained per-element scroll ([`element_scroll`](Self::element_scroll),
    /// driven by [`scroll_at`](Self::scroll_at)) with a caller's own offsets — the
    /// caller's winning on a key collision (a host that explicitly positions a
    /// container, e.g. meerkat's panes, overrides the wheel-driven offset). The
    /// merge the hit-test and paint both read, so a content document's inner
    /// scrollers move while a host's explicit offsets still apply. Cheap-paths the
    /// common cases (an empty side returns the other cloned), so callers that never
    /// scroll a nested container pay one clone of their own (usually empty) map.
    fn merged_scroll(&self, host: &ScrollOffsets<Id>) -> ScrollOffsets<Id> {
        if self.element_scroll.is_empty() {
            return host.clone();
        }
        if host.is_empty() {
            return self.element_scroll.clone();
        }
        let mut merged = self.element_scroll.clone();
        for (k, v) in host {
            merged.insert(*k, *v);
        }
        merged
    }

    /// Scroll the nearest nested scroll container under scene point `(x, y)` by
    /// `(dx, dy)` device px — the wheel default action for `overflow: scroll/auto`
    /// containers. Hit-tests the point (element-scroll aware), then walks hit → root
    /// for the nearest ancestor that scrolls on a requested axis and is not already
    /// at its limit (CSS scroll *chaining*: a container pinned at its edge passes the
    /// delta to its scrollable ancestor), updates that container's retained
    /// [`element_scroll`](Self::element_scroll) offset, and returns `true`. With no
    /// scrollable container in the chain it falls through to the document viewport
    /// ([`scroll_by`](Self::scroll_by)), returning whether the document moved. The
    /// host maps a wheel delta straight onto this; the next
    /// [`emit_paint_list`](Self::emit_paint_list) paints at the new offsets.
    pub fn scroll_at<D>(&mut self, dom: &D, x: f32, y: f32, dx: f32, dy: f32) -> bool
    where
        D: LayoutDom<NodeId = Id>,
    {
        // Hit-test through the current element scroll (so a click on already-scrolled
        // content resolves to the right node); the host passes no extra offset here.
        let mut node = self.hit_test(dom, x, y, &ScrollOffsets::default());
        while let Some(n) = node {
            if let Some(next) = self.scroll_step(dom, n, dx, dy) {
                self.element_scroll.insert(n, next);
                return true;
            }
            node = dom.parent(n);
        }
        // No nested scroll container consumed the delta → scroll the document.
        let before = self.viewport.scroll;
        self.scroll_by(dom, dx, dy);
        self.viewport.scroll != before
    }

    /// The clamped new element-scroll offset for scrolling `node` by `(dx, dy)` from
    /// its current offset, or `None` when `node` is not a wheel-scrollable container
    /// on either requested axis, or the clamp leaves the offset unchanged (already at
    /// its limit — the caller chains to the next ancestor). A non-scrollable axis
    /// holds its current value; a scrollable axis clamps to `[0, extent]` from
    /// [`scroll_extent`](Self::scroll_extent).
    fn scroll_step<D>(&self, dom: &D, node: Id, dx: f32, dy: f32) -> Option<(f32, f32)>
    where
        D: LayoutDom<NodeId = Id>,
    {
        let cv = crate::paint_emit::primary_cv(&self.styles, node)?;
        let sx = crate::paint_emit::scrolls_overflow_x(&cv);
        let sy = crate::paint_emit::scrolls_overflow_y(&cv);
        if !sx && !sy {
            return None;
        }
        let (mx, my) = self.scroll_extent(dom, node);
        let cur = self.element_scroll.get(&node).copied().unwrap_or((0.0, 0.0));
        let nx = if sx { (cur.0 + dx).clamp(0.0, mx) } else { cur.0 };
        let ny = if sy { (cur.1 + dy).clamp(0.0, my) } else { cur.1 };
        if (nx - cur.0).abs() < f32::EPSILON && (ny - cur.1).abs() < f32::EPSILON {
            return None;
        }
        Some((nx, ny))
    }

    /// The maximum element-scroll offset `(mx, my)` for scroll container `node`: per
    /// axis, how far its content overflows its scrollport (the padding box) before
    /// the content's far edge reaches the scrollport edge. The nested-container
    /// analogue of [`document_scroll_range`], rooted at the container and measured in
    /// its border-box coordinates (the frame paint and hit-test position descendants
    /// in). `(0, 0)` when `node` has no fragment or its content does not overflow.
    ///
    /// First cut of the scrollable-overflow region: the union of in-flow + `absolute`
    /// descendant fragment far edges plus the container's end padding, skipping
    /// `position: fixed` subtrees and not descending past a nested clip container (its
    /// own box bounds its descendants). The precise region (rule 4: transformed /
    /// negative-margin descendant overflow) is a documented follow-on.
    fn scroll_extent<D>(&self, dom: &D, node: Id) -> (f32, f32)
    where
        D: LayoutDom<NodeId = Id>,
    {
        let Some(frame) = self.fragments.rect_of(node) else {
            return (0.0, 0.0);
        };
        let (cr, cb) = self.content_far_edge(dom, node);
        // The scrollport is the padding box (border-box minus borders); the content
        // can scroll until its far edge plus the container's end padding reaches it.
        let port_right = frame.size.width - frame.border.right;
        let port_bottom = frame.size.height - frame.border.bottom;
        let mx = (cr + frame.padding.right - port_right).max(0.0);
        let my = (cb + frame.padding.bottom - port_bottom).max(0.0);
        (mx, my)
    }

    /// The far (right, bottom) edge of `node`'s in-flow + `absolute` descendants in
    /// `node`'s border-box coordinates — the content extent
    /// [`scroll_extent`](Self::scroll_extent) measures against. Children are
    /// positioned relative to the container's border-box origin (paint walks them
    /// there, `paint_emit.rs`), so accumulation starts at `(0, 0)` and excludes the
    /// container's own box (it is the scrollport, not content).
    fn content_far_edge<D>(&self, dom: &D, node: Id) -> (f32, f32)
    where
        D: LayoutDom<NodeId = Id>,
    {
        let mut extent = (0.0f32, 0.0f32);
        for child in dom.dom_children(node) {
            self.accumulate_far_edge(dom, child, (0.0, 0.0), &mut extent);
        }
        extent
    }

    /// Accumulate the far (right, bottom) edge of `id`'s fragment and its scrollable
    /// descendants into `extent`, in the host container's border-box coordinates
    /// (`parent_origin` accumulated through the DOM, as paint/hit-test do). Mirrors
    /// [`viewport::extend_scrollable`](crate::viewport) one level down: a `position:
    /// fixed` subtree is viewport-anchored (it never scrolls the container), and a
    /// nested overflow-clip container bounds its own descendants.
    fn accumulate_far_edge<D>(
        &self,
        dom: &D,
        id: Id,
        parent_origin: (f32, f32),
        extent: &mut (f32, f32),
    ) where
        D: LayoutDom<NodeId = Id>,
    {
        let cv = crate::paint_emit::primary_cv(&self.styles, id);
        if cv.as_deref().is_some_and(crate::paint_emit::is_fixed) {
            return;
        }
        let origin = match self.fragments.rect_of(id) {
            Some(l) => {
                let ox = parent_origin.0 + l.location.x;
                let oy = parent_origin.1 + l.location.y;
                extent.0 = extent.0.max(ox + l.size.width);
                extent.1 = extent.1.max(oy + l.size.height);
                (ox, oy)
            },
            None => parent_origin,
        };
        if cv.as_deref().is_some_and(crate::paint_emit::clips_overflow) {
            return;
        }
        for child in dom.dom_children(id) {
            self.accumulate_far_edge(dom, child, origin, extent);
        }
    }

    /// Recompute the viewport's propagated overflow + size after a relayout,
    /// preserving the host's scroll re-clamped to the new content (a relayout can
    /// shrink the page under the current offset). Called on every layout-changing
    /// path, not the hot `RepaintOnly` frame.
    fn recompute_viewport<D>(&mut self, dom: &D)
    where
        D: LayoutDom<NodeId = Id>,
    {
        let size = DeviceIntSize::new(self.width as i32, self.height as i32);
        let prev_scroll = self.viewport.scroll;
        let mut vp = Viewport::for_document(dom, &self.styles, size);
        let range = document_scroll_range(dom, &self.styles, &self.fragments, size);
        vp.scroll = vp.clamp_scroll(prev_scroll, range);
        self.viewport = vp;
    }

    /// The aggregate `RestyleDamage` from the most recent attribute-only
    /// [`apply`](Self::apply) (`empty()` before any, and unchanged by a
    /// structural batch, which takes the cascade-from-scratch path).
    pub fn last_damage(&self) -> RestyleDamage {
        self.last_damage
    }

    /// Enforce the fixed-stylesheet invariant in debug builds. The persistent
    /// Stylist is built once from `new()`'s stylesheets and cannot be safely
    /// rebuilt mid-session (the prior passes' rule nodes, held on `ElementData`,
    /// would dangle into a dropped tree). A caller that changes the set between
    /// `apply()` calls is silently restyling against the old sheets — catch it
    /// loudly in debug. No-op in release (the cost is a `Vec<String>` compare).
    fn debug_assert_fixed_sheets(&self, stylesheets: &[&str]) {
        debug_assert!(
            stylesheets.len() == self.sheets.len()
                && stylesheets.iter().zip(&self.sheets).all(|(a, b)| a == b),
            "IncrementalLayout stylesheets are fixed at new(); the persistent rule \
             tree cannot be rebuilt mid-session (hot-reload is a follow-up). Got a \
             different set in apply().",
        );
    }

    /// Apply a drained mutation batch, updating styles (and fragments
    /// when geometry changed). Returns what path was taken.
    ///
    /// - **Attribute-only batch:** incremental restyle via Stylo
    ///   invalidation; re-layout only if the restyle damage requires it,
    ///   else paint-only (fragments untouched).
    /// - **Any structural mutation:** full cascade + layout (correct,
    ///   conservative — structural invalidation is the relayout-scope
    ///   path's job, not the attribute/state invalidator's).
    pub fn apply<D>(
        &mut self,
        dom: &D,
        stylesheets: &[&str],
        mutations: &[DomMutation<Id>],
    ) -> Applied
    where
        D: LayoutDom<NodeId = Id>,
    {
        if mutations.is_empty() {
            return Applied::Unchanged;
        }
        self.debug_assert_fixed_sheets(stylesheets);

        let attribute_only = mutations
            .iter()
            .all(|m| matches!(m, DomMutation::AttributeChanged { .. }));

        if !attribute_only {
            return self.apply_structural(dom, mutations);
        }

        // Attribute-only → incremental restyle over the persistent plane,
        // reusing the persistent Stylist (whose rule tree the prior pass's rule
        // nodes live in — the precondition for the cheap replacement path).
        let outcome = restyle_with_snapshots(dom, &mut self.styles, &self.stylist, mutations);
        self.last_damage = outcome.damage;
        if outcome.needs_relayout {
            let (fragments, built) =
                full_layout(dom, &self.styles, self.width, self.height, &mut self.text_ctx);
            self.fragments = fragments;
            self.built = built;
            self.paint_side_valid = true;
            self.images = ImagePlane::decode_from_dom(dom);
            self.recompute_viewport(dom);
            Applied::Restyled
        } else {
            // Paint-only: prior fragments (and box-tree side-table) still valid.
            // But paint reads each box node's cached `style` (the box-tree paint
            // re-root), and this path keeps the prior box tree — so refresh the
            // mutated elements' cached style from the re-cascaded plane. Without
            // it a paint-tier change (transform / color) reaches `self.styles` but
            // never the emit, freezing the orrery's per-frame motion until a
            // relayout (a host resize, which rebuilds the tree).
            let mutated = mutations.iter().filter_map(|m| match m {
                DomMutation::AttributeChanged { node, .. } => Some(*node),
                _ => None,
            });
            self.built.refresh_styles_for(&self.styles, mutated);
            Applied::RepaintOnly
        }
    }

    /// Emit a glyph-bearing [`ServalPaintList`] from the current layout — the
    /// engine-agnostic command stream a host composites or lowers to a scene.
    /// Valid on the `RepaintOnly` path (a transform-only frame keeps box
    /// geometry, so the retained box tree + text context still match the
    /// fragments). Paints the session's decoded `<img>` images (data: URIs,
    /// refreshed at each full layout), so `<img>` content like the chrome favicons
    /// appears on the cheap path; CSS `background-image` is not planed here yet.
    ///
    /// Requires the last [`apply`](Self::apply) to have been non-structural (a
    /// structural splice updates fragments but not the box-tree side-table); the
    /// pre-materialized-pool host that drives this only ever sends attribute-only
    /// (transform) batches, so it never trips the assert.
    pub fn emit_paint_list<D>(
        &self,
        dom: &D,
        scroll_offsets: &ScrollOffsets<Id>,
        viewport: DeviceIntSize,
    ) -> ServalPaintList
    where
        D: LayoutDom<NodeId = Id>,
    {
        debug_assert!(
            self.paint_side_valid,
            "emit_paint_list after a structural splice: the box-tree side-table is \
             stale (relayout first). Attribute-only hosts never hit this.",
        );
        let bg_images = BackgroundImagePlane::new();
        // Merge the session's retained per-element scroll (driven by `scroll_at`)
        // with the caller's own offsets, so a content document's inner scrollers
        // move while a host's explicit offsets (meerkat's panes) still apply.
        let merged = self.merged_scroll(scroll_offsets);
        // Paint at the session's document scroll (the viewport the host drives via
        // `set_viewport_scroll`); `(0,0)` until the host scrolls, so existing
        // consumers that never scroll the document are unchanged.
        emit_paint_list_scrolled(
            dom,
            &self.styles,
            &self.fragments,
            &self.built,
            &self.text_ctx,
            &self.images,
            &bg_images,
            &merged,
            viewport,
            self.viewport.scroll,
        )
    }

    /// The caret rectangle for `byte_offset` within `node`'s laid-out text, in
    /// absolute scene coordinates, served from the session's **retained** layout
    /// (the same `built` / `text_ctx` / `fragments` [`emit_paint_list`](Self::emit_paint_list)
    /// paints from) — no re-cascade. `None` if `node` has no cached text layout.
    /// The session companion to `LaidOutDocument::caret_screen_rect`: a host that
    /// overlays a focused field's caret reads it from the same session it renders
    /// through. Valid whenever `emit_paint_list` is (post full layout / the
    /// `RepaintOnly` path); a structural splice invalidates the box-tree side-table.
    pub fn caret_rect<D>(
        &self,
        dom: &D,
        node: Id,
        byte_offset: usize,
        width: f32,
    ) -> Option<crate::caret::CaretRect>
    where
        D: LayoutDom<NodeId = Id>,
    {
        crate::caret::caret_rect(
            dom,
            node,
            byte_offset,
            &self.built,
            &self.text_ctx,
            &self.fragments,
            width,
        )
    }

    /// The selection-highlight rectangles for the byte range `[start, end)` within
    /// `node`'s laid-out text, in absolute scene coordinates, served from the
    /// session's retained layout. Empty when collapsed or `node` has no cached
    /// text layout. The selection companion to [`caret_rect`](Self::caret_rect),
    /// for the same focused-field overlay.
    pub fn selection_rects<D>(
        &self,
        dom: &D,
        node: Id,
        start: usize,
        end: usize,
    ) -> Vec<crate::caret::CaretRect>
    where
        D: LayoutDom<NodeId = Id>,
    {
        crate::caret::selection_rects(
            dom,
            node,
            start,
            end,
            &self.built,
            &self.text_ctx,
            &self.fragments,
        )
    }

    /// The `::selection` background / foreground colors in effect at `node`
    /// (walking to the nearest ancestor that sets them), resolved from the
    /// session's retained [`StylePlane`] — `None` when no `::selection` rule
    /// applies, so the caller falls back to its theme default highlight. The
    /// session companion to the stateless overlay's `selection_style`, so a
    /// session-rendered field highlights selection the same as the stateless path.
    pub fn selection_style<D>(&self, dom: &D, node: Id) -> Option<([f32; 4], [f32; 4])>
    where
        D: LayoutDom<NodeId = Id>,
    {
        crate::caret::selection_style(dom, &self.styles, node)
    }

    /// The caret colour for `node` (the cascaded text colour — `caret-color: auto`),
    /// resolved from the session's retained [`StylePlane`]. `None` when the node
    /// has no style data, so the host keeps its theme default. The session
    /// companion to [`caret::caret_color`](crate::caret::caret_color).
    pub fn caret_color<D>(&self, dom: &D, node: Id) -> Option<[f32; 4]>
    where
        D: LayoutDom<NodeId = Id>,
    {
        crate::caret::caret_color(dom, &self.styles, node)
    }

    /// Structural batch: re-cascade styles (full — structural
    /// restyle-invalidation is the deferred optimization), then lay out
    /// **incrementally** by re-laying-out each coalesced subtree over the
    /// fresh styles and splicing it into the prior fragments at its real
    /// position. Falls back to a full layout when a subtree's outer size
    /// changed (ancestors would reflow) or a root wasn't previously laid
    /// out — the same boundary the coarse-oracle diff-test guards.
    fn apply_structural<D>(
        &mut self,
        dom: &D,
        mutations: &[DomMutation<Id>],
    ) -> Applied
    where
        D: LayoutDom<NodeId = Id>,
    {
        // Plan the affected subtree roots (shared by the partial cascade
        // and the layout splice).
        let invalidations: Vec<_> = mutations.iter().map(classify).collect();
        let roots = coalesce(&invalidations, |id| dom.parent(id));
        let root_ids: Vec<Id> = roots.iter().map(|inv| inv.node()).collect();

        // 1. Styles: partial cascade — re-cascade only the affected
        //    subtrees over the persistent plane (the inserted/replaced
        //    nodes + within-parent sibling/nth-child effects).
        restyle_structural(dom, &mut self.styles, &self.stylist, &root_ids);

        // 2. Fragments: incremental layout splice over the restyled plane.

        let mut result = self.fragments.clone();
        for inv in &roots {
            let root = inv.node();
            let Some(prior_root) = self.fragments.rect_of(root).copied() else {
                return self.full_relayout(dom);
            };
            // Lay out just this subtree (re-rooted) over the persistent styles.
            let scoped = lay_out(&SubtreeView::new(dom, root), &self.styles, self.width, self.height);
            let Some(scoped_root) = scoped.rect_of(root).copied() else {
                return self.full_relayout(dom);
            };
            // Margin-collapse parity at the splice boundary. A `SubtreeView`-rooted
            // scoped layout makes `root` the scoped ICB — a block formatting context
            // — so its first/last in-flow child margins stop collapsing INTO it. In
            // the full document a non-BFC `root` (e.g. `<body>`, a plain `<div>`) has
            // those margins collapse through it, shifting its children. Splicing such
            // a root would mis-place every child by the lost collapse, so fall back to
            // a correct full relayout. (CSS 2.2 §8.3.1; for a shallow root like
            // `<body>` the full relayout is barely more than the subtree layout.)
            if splice_loses_margin_collapse(dom, &self.styles, &scoped, root) {
                return self.full_relayout(dom);
            }
            // Outer size change → ancestors would reflow → full fallback.
            if (scoped_root.size.width - prior_root.size.width).abs() >= 0.5
                || (scoped_root.size.height - prior_root.size.height).abs() >= 0.5
            {
                return self.full_relayout(dom);
            }
            // Splice the scoped subtree into the prior fragments. Fragment
            // locations are *parent-relative* (Taffy's `final_layout.location`;
            // `caret::absolute_origin` walks to accumulate), so a descendant's
            // scoped location — relative to its own parent inside the subtree — is
            // already its real location: the size-preserving precondition + the
            // margin-collapse guard above make the subtree's internal layout
            // context-independent, so the scoped pass reproduces it exactly. Keep
            // descendants as-is; only the root's own parent-relative location lives
            // outside the subtree (the scoped pass put it at the scoped origin), so
            // pin it to its prior value. (Translating descendants by the root delta
            // would force them into absolute space, diverging from the full path
            // whenever an ancestor carries an offset, e.g. the UA `body` margin.)
            let mut subtree = Vec::new();
            collect_subtree(dom, root, &mut subtree);
            for node in subtree {
                if let Some(layout) = scoped.rect_of(node) {
                    let mut placed = *layout;
                    if node == root {
                        placed.location = prior_root.location;
                    }
                    result.insert(node, placed);
                }
            }
        }
        self.fragments = result;
        // The splice updates fragments but not the box-tree side-table, so a
        // following emit_paint_list would mismatch — mark it stale (a relayout
        // re-validates). Attribute-only hosts (the pool) never take this path.
        self.paint_side_valid = false;
        self.recompute_viewport(dom);
        Applied::Spliced
    }

    /// Full layout over the current (already-cascaded) styles. The
    /// fallback for the structural splice.
    fn full_relayout<D>(&mut self, dom: &D) -> Applied
    where
        D: LayoutDom<NodeId = Id>,
    {
        let (fragments, built) =
            full_layout(dom, &self.styles, self.width, self.height, &mut self.text_ctx);
        self.fragments = fragments;
        self.built = built;
        self.paint_side_valid = true;
        self.images = ImagePlane::decode_from_dom(dom);
        self.recompute_viewport(dom);
        Applied::FullRecompute
    }
}

/// Pre-order subtree node ids rooted at `root`.
fn collect_subtree<D: LayoutDom>(dom: &D, root: D::NodeId, out: &mut Vec<D::NodeId>) {
    out.push(root);
    for child in dom.dom_children(root) {
        collect_subtree(dom, child, out);
    }
}

/// Whether splicing the subtree rooted at `root` would lose a margin collapse
/// that the full document performs — the staleness check behind the splice's
/// full-relayout fallback (CSS 2.2 §8.3.1, §9.4.1).
///
/// A `SubtreeView`-rooted scoped layout makes `root` the scoped ICB, hence a
/// block formatting context: its first/last in-flow child margins are *applied*
/// (no collapse into `root`). But if `root` is NOT a BFC in the full document,
/// those margins collapse *through* it there, so the scoped child positions are
/// off by the lost collapse. True exactly when `root` is collapse-permeable on a
/// block edge AND an adjacent in-flow child carries the margin that would
/// collapse across it (so a margin-free subtree still splices cheaply).
fn splice_loses_margin_collapse<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    scoped: &FragmentPlane<D::NodeId>,
    root: D::NodeId,
) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if establishes_independent_formatting_context(styles, root) {
        return false;
    }
    let in_flow: Vec<D::NodeId> = dom
        .dom_children(root)
        .filter(|&c| scoped.rect_of(c).is_some() && is_in_flow(styles, c))
        .collect();
    let (Some(&first), Some(&last)) = (in_flow.first(), in_flow.last()) else {
        return false;
    };
    let first_top = scoped.rect_of(first).map_or(0.0, |l| l.margin.top);
    let last_bottom = scoped.rect_of(last).map_or(0.0, |l| l.margin.bottom);
    first_top.abs() > 0.5 || last_bottom.abs() > 0.5
}

/// Whether `node` establishes an independent formatting context, so its in-flow
/// children's margins do not collapse into it: `overflow != visible`, an
/// out-of-flow box, or a non-`Flow` inner display (`flow-root` / `flex` / `grid`
/// / `table`). Conservatively `false` when the style is unavailable (defer to
/// the child-margin check rather than splice blindly).
fn establishes_independent_formatting_context<NodeId>(
    styles: &StylePlane<NodeId>,
    node: NodeId,
) -> bool
where
    NodeId: Copy + Eq + Hash,
{
    use style::values::computed::{Overflow, PositionProperty};
    use style::values::specified::box_::DisplayInside;
    let Some(cv) = crate::paint_emit::primary_cv(styles, node) else {
        return false;
    };
    let b = cv.get_box();
    !matches!(b.overflow_x, Overflow::Visible)
        || !matches!(b.overflow_y, Overflow::Visible)
        || matches!(b.position, PositionProperty::Absolute | PositionProperty::Fixed)
        || !matches!(b.display.inside(), DisplayInside::Flow)
}

/// Whether `node` is in normal flow (not absolutely/fixed positioned). Floats
/// are treated as in-flow here: counting one only risks an unnecessary splice
/// fallback, never an incorrect splice.
fn is_in_flow<NodeId>(styles: &StylePlane<NodeId>, node: NodeId) -> bool
where
    NodeId: Copy + Eq + Hash,
{
    use style::values::computed::PositionProperty;
    let Some(cv) = crate::paint_emit::primary_cv(styles, node) else {
        return false;
    };
    !matches!(cv.get_box().position, PositionProperty::Absolute | PositionProperty::Fixed)
}

/// The first element (pre-order) whose `id` attribute equals `id`, or `None` — the
/// anchor-fragment target lookup behind [`IncrementalLayout::scroll_to_id`].
fn element_by_id<D: LayoutDom>(dom: &D, id: &str) -> Option<D::NodeId> {
    use html5ever::{local_name, ns};
    let mut stack = vec![dom.document()];
    while let Some(node) = stack.pop() {
        if dom.attribute(node, &ns!(), &local_name!("id")) == Some(id) {
            return Some(node);
        }
        stack.extend(dom.dom_children(node));
    }
    None
}

/// The fragment of an in-page link: `node`'s `#id` href without the `#`, or `None`
/// when `node` is not an `<a>` with an in-page (`#…`) href. Behind
/// [`IncrementalLayout::link_fragment_at`].
fn anchor_fragment<D: LayoutDom>(dom: &D, node: D::NodeId) -> Option<String> {
    use html5ever::{local_name, ns};
    if dom.element_name(node)?.local != local_name!("a") {
        return None;
    }
    let href = dom.attribute(node, &ns!(), &local_name!("href"))?;
    href.strip_prefix('#').filter(|f| !f.is_empty()).map(str::to_string)
}

/// The full, non-empty `href` of an `<a>` element (in-page or cross-document), or
/// `None` when `node` is not such a link. Behind [`IncrementalLayout::link_href_at`]
/// and the all-anchors harvest ([`crate::link_harvest`]).
pub(crate) fn anchor_href<D: LayoutDom>(dom: &D, node: D::NodeId) -> Option<String> {
    use html5ever::{local_name, ns};
    if dom.element_name(node)?.local != local_name!("a") {
        return None;
    }
    let href = dom.attribute(node, &ns!(), &local_name!("href"))?;
    (!href.is_empty()).then(|| href.to_string())
}

/// Lay out over an already-cascaded plane (no images in the scripted
/// path), hiding the taffy viewport type.
fn lay_out<D>(dom: &D, styles: &StylePlane<D::NodeId>, width: f32, height: f32) -> FragmentPlane<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync,
{
    // Scoped-splice fallback path (fragments only): a throwaway context is fine
    // here; the session's persistent one rides the `full_layout` relayout paths.
    let mut text_ctx = TextMeasureCtx::new();
    full_layout(dom, styles, width, height, &mut text_ctx).0
}

/// Full layout into the session's retained `text_ctx` (reset per pass, font
/// context reused), returning fragments **and** the box tree the paint-emit pass
/// needs. The session keeps both plus `text_ctx` so it can emit without
/// re-laying-out, and reuses one font context for its whole life.
fn full_layout<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    width: f32,
    height: f32,
    text_ctx: &mut TextMeasureCtx,
) -> (FragmentPlane<D::NodeId>, BoxTree<D::NodeId>)
where
    D: LayoutDom,
    // Propagated for `layout_via_box_tree`'s parallel shaping pre-pass.
    D::NodeId: Copy + Eq + Hash + Send + Sync,
{
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width),
        height: taffy::AvailableSpace::Definite(height),
    };
    crate::box_tree::layout_via_box_tree(dom, styles, &images, viewport, text_ctx)
}

#[cfg(test)]
mod tests {
    use html5ever::ns;
    use layout_dom_api::{LayoutDomMut, QualName};
    use serval_scripted_dom::ScriptedDom;

    use super::*;
    use crate::cascade::run_cascade;

    const W: f32 = 800.0;
    const H: f32 = 600.0;

    fn html(l: &str) -> QualName {
        QualName::new(None, ns!(html), l.into())
    }
    fn attr(l: &str) -> QualName {
        QualName::new(None, ns!(), l.into())
    }

    /// The text color a node's persistent plane resolved to.
    fn color(layout: &IncrementalLayout<<ScriptedDom as LayoutDom>::NodeId>, id: <ScriptedDom as LayoutDom>::NodeId) -> [f32; 4] {
        let entry = layout.styles.get(id).expect("entry");
        let data = entry.borrow_data().expect("data");
        *data.styles.primary().get_inherited_text().color.into_srgb_legacy().raw_components()
    }

    fn drain(dom: &mut ScriptedDom) -> Vec<DomMutation<<ScriptedDom as LayoutDom>::NodeId>> {
        let mut v = Vec::new();
        dom.drain_mutations(&mut v);
        v
    }

    /// A color-only change: incremental restyle, layout skipped
    /// (`RepaintOnly`), the `<p>` recolors, and its rect is unchanged
    /// (color doesn't move boxes).
    #[test]
    fn color_change_is_repaint_only_and_skips_layout() {
        const SHEET: &[&str] =
            &["p{width:100px;height:20px}.red{color:rgb(255,0,0)}.blue{color:rgb(0,0,255)}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.set_attribute(p, attr("class"), "red");
        dom.append_child(body, p);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let rect_before = *layout.fragments().rect_of(p).expect("p rect");
        assert!((color(&layout, p)[0] - 1.0).abs() < 0.001, "p starts red");

        // Swap class red → blue.
        let _ = drain(&mut dom);
        dom.set_attribute(p, attr("class"), "blue");
        let muts = drain(&mut dom);
        let applied = layout.apply(&dom, SHEET, &muts);

        assert_eq!(applied, Applied::RepaintOnly, "color swap should skip layout");
        assert!((color(&layout, p)[2] - 1.0).abs() < 0.001, "p should be blue after restyle");
        let rect_after = *layout.fragments().rect_of(p).expect("p rect");
        assert_eq!(rect_before, rect_after, "color change must not move the box");
    }

    /// `emit_paint_list` produces a glyph-bearing list, and keeps producing one
    /// after a transform-only (`RepaintOnly`) move — the bridge a per-frame
    /// orrery host rides: emit the moved scene without a relayout.
    #[test]
    fn emit_paint_list_survives_a_repaint_only_transform() {
        use paint_list_api::{PaintCmd, PaintList};

        const SHEET: &[&str] = &["p{width:40px;height:40px}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.set_attribute(p, attr("style"), "transform: translate(10px, 0px)");
        dom.append_child(body, p);
        let text = dom.create_text("hi");
        dom.append_child(p, text);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let scroll = ScrollOffsets::default();
        let dev = DeviceIntSize::new(W as i32, H as i32);
        let has_glyphs = |pl: &ServalPaintList| {
            pl.commands()
                .iter()
                .any(|c| matches!(c, PaintCmd::DrawText(t) if !t.glyphs.is_empty()))
        };

        assert!(has_glyphs(&layout.emit_paint_list(&dom, &scroll, dev)), "emits text initially");

        // Transform-only change → RepaintOnly; emit must still produce the scene.
        let _ = drain(&mut dom);
        dom.set_attribute(p, attr("style"), "transform: translate(90px, 0px)");
        let muts = drain(&mut dom);
        assert_eq!(
            layout.apply(&dom, SHEET, &muts),
            Applied::RepaintOnly,
            "a transform-only change is paint-tier",
        );
        assert!(
            has_glyphs(&layout.emit_paint_list(&dom, &scroll, dev)),
            "emit still produces the glyph-bearing scene after the RepaintOnly move",
        );
    }

    /// REGRESSION GUARD (orrery freeze): after a RepaintOnly inline-transform
    /// change, emit must carry the NEW translate — not the value baked into the
    /// box tree at full layout. Paint reads `BoxNode::style` (the §5 box-tree
    /// re-root); this path keeps the prior box tree, so unless the changed nodes'
    /// cached style is refreshed from the re-cascaded plane, the painted transform
    /// stays frozen until a relayout (a window resize, for the orrery host). The
    /// sibling `emit_paint_list_survives_*` only checks glyph presence, so it
    /// passed even while the position was stale; this asserts the position moves.
    #[test]
    fn repaint_only_transform_moves_the_emitted_translate() {
        use paint_list_api::{PaintCmd, PaintList};

        const SHEET: &[&str] = &[".n{position:absolute;width:80px;height:40px}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let node = dom.create_element(html("div"));
        dom.set_attribute(node, attr("class"), "n");
        dom.set_attribute(node, attr("style"), "transform:translate(10px,0px)"); // materialized
        dom.append_child(body, node);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let scroll = ScrollOffsets::default();
        let dev = DeviceIntSize::new(W as i32, H as i32);
        // The node's CSS transform folds into a `PushTransform` (m41 = translate-x);
        // the html/body pushes are identity (m41 = 0), so the max picks the node's.
        let translate_x = |pl: &ServalPaintList| {
            pl.commands()
                .iter()
                .filter_map(|c| match c {
                    PaintCmd::PushTransform(spec) => Some(spec.transform.m41),
                    _ => None,
                })
                .fold(f32::MIN, f32::max)
        };

        let before = translate_x(&layout.emit_paint_list(&dom, &scroll, dev));
        assert!((before - 10.0).abs() < 0.5, "starts at translate-x 10, got {before}");

        // Transform-only change → RepaintOnly; the emitted translate must follow.
        let _ = drain(&mut dom);
        dom.set_attribute(node, attr("style"), "transform:translate(90px,0px)");
        let muts = drain(&mut dom);
        assert_eq!(
            layout.apply(&dom, SHEET, &muts),
            Applied::RepaintOnly,
            "a transform-only change is paint-tier",
        );
        let after = translate_x(&layout.emit_paint_list(&dom, &scroll, dev));
        assert!(
            (after - 90.0).abs() < 0.5,
            "RepaintOnly emit must carry the NEW translate-x (90), got {after}",
        );
    }

    /// A width change: incremental restyle that re-lays-out
    /// (`Restyled`), and the new rect matches a full cascade + layout.
    #[test]
    fn width_change_restyles_and_relayouts_matching_full() {
        const SHEET: &[&str] =
            &["p{height:20px}.narrow{width:50px}.wide{width:200px}"];
        let build = || {
            let mut dom = ScriptedDom::new();
            let root = dom.document();
            let h = dom.create_element(html("html"));
            dom.append_child(root, h);
            let body = dom.create_element(html("body"));
            dom.append_child(h, body);
            let p = dom.create_element(html("p"));
            dom.set_attribute(p, attr("class"), "narrow");
            dom.append_child(body, p);
            (dom, p)
        };

        let (mut dom, p) = build();
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        assert!((layout.fragments().rect_of(p).unwrap().size.width - 50.0).abs() < 0.5);

        let _ = drain(&mut dom);
        dom.set_attribute(p, attr("class"), "wide");
        let muts = drain(&mut dom);
        let applied = layout.apply(&dom, SHEET, &muts);
        assert_eq!(applied, Applied::Restyled, "width change should re-layout");

        // Oracle: a fresh full cascade + layout of the mutated DOM.
        let mut oracle_styles = StylePlane::new();
        run_cascade(&dom, &mut oracle_styles, euclid::Size2D::new(W, H), SHEET, None);
        let oracle = lay_out(&dom, &oracle_styles, W, H);

        let inc = layout.fragments().rect_of(p).unwrap();
        let full = oracle.rect_of(p).unwrap();
        assert!((inc.size.width - full.size.width).abs() < 0.5, "width must match full layout");
        assert!((inc.size.width - 200.0).abs() < 0.5, "p should be 200px wide after restyle");
    }

    /// A structural change whose subtree keeps its outer size splices
    /// incrementally (`Spliced`): appending a `<p>` under a fixed-height `<body>`
    /// re-lays-out the body subtree (its outer size unchanged, so it splices), and
    /// the new `<p>` lands where a full recompute would put it. (The body is sized
    /// explicitly here — its UA height is `auto`, which a content append would grow,
    /// taking the full-recompute fallback instead. It is also `overflow: hidden` so
    /// it establishes a BFC: under the UA `p { margin }`, a non-BFC body would take
    /// the margin-collapse fallback — see `margined_first_child_falls_back_to_full`.)
    #[test]
    fn structural_change_splices_incrementally() {
        const SHEET: &[&str] = &["body { height: 200px; overflow: hidden; } p { height: 20px; }"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);
        let muts = drain(&mut dom);
        assert_eq!(layout.apply(&dom, SHEET, &muts), Applied::Spliced);

        // The new <p> matches a full cascade + layout of the mutated DOM.
        let mut oracle_styles = StylePlane::new();
        run_cascade(&dom, &mut oracle_styles, euclid::Size2D::new(W, H), SHEET, None);
        let oracle = lay_out(&dom, &oracle_styles, W, H);
        let spliced = layout.fragments().rect_of(p).expect("new <p> laid out");
        let full = oracle.rect_of(p).expect("oracle <p>");
        assert!((spliced.location.y - full.location.y).abs() < 0.5, "spliced <p> y must match full");
        assert!((spliced.size.height - full.size.height).abs() < 0.5, "spliced <p> height must match full");
    }

    /// Margin-collapse parity (fix B): a fixed-size, non-BFC subtree root
    /// (`<body>`) whose first in-flow child carries a block-start margin does
    /// NOT splice — the scoped `SubtreeView` layout makes `body` a BFC, so that
    /// margin would stop collapsing through it and the child would land too low.
    /// The engine detects the lost collapse and falls back to a correct full
    /// recompute, so the `<p>` lands exactly where full layout puts it.
    #[test]
    fn margined_first_child_falls_back_to_full() {
        const SHEET: &[&str] = &["body { height: 200px; } p { height: 20px; margin: 16px 0; }"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);
        let muts = drain(&mut dom);
        // Non-BFC body + margined first child → splice is unsound → full recompute.
        assert_eq!(layout.apply(&dom, SHEET, &muts), Applied::FullRecompute);

        // And the result matches a from-scratch full layout (the collapse is honored).
        let mut oracle_styles = StylePlane::new();
        run_cascade(&dom, &mut oracle_styles, euclid::Size2D::new(W, H), SHEET, None);
        let oracle = lay_out(&dom, &oracle_styles, W, H);
        let got = layout.fragments().rect_of(p).expect("new <p> laid out");
        let want = oracle.rect_of(p).expect("oracle <p>");
        assert!((got.location.y - want.location.y).abs() < 0.5, "p y must match full: {} vs {}", got.location.y, want.location.y);
    }

    /// When a structural change grows its subtree's outer size (an
    /// auto-height container gains a child), ancestors would reflow, so
    /// the engine falls back to a full recompute.
    #[test]
    fn structural_size_growth_falls_back_to_full() {
        // `div` is auto-height (no height rule) → grows with its children.
        const SHEET: &[&str] = &["div{width:50px}p{height:20px}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let div = dom.create_element(html("div"));
        dom.append_child(body, div);
        let p1 = dom.create_element(html("p"));
        dom.append_child(div, p1);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);
        // Append a second <p> → the div grows from 20px to 40px tall.
        let p2 = dom.create_element(html("p"));
        dom.append_child(div, p2);
        let muts = drain(&mut dom);
        assert_eq!(layout.apply(&dom, SHEET, &muts), Applied::FullRecompute);
        assert!(layout.fragments().rect_of(p2).is_some(), "new <p> laid out after fallback");
    }

    /// The partial structural cascade re-matches **existing** siblings,
    /// not just the new node: with `p:last-child { color: red }`,
    /// appending a `<p>` must recolor the previously-last `<p>` (now black)
    /// and color the new one red — matching a full re-cascade. This is the
    /// receipt that `restyle_structural`'s `RESTYLE_DESCENDANTS` re-runs
    /// `:last-child` over the parent's children, not only the insertion.
    #[test]
    fn structural_resibling_recolors_existing_via_partial_cascade() {
        const SHEET: &[&str] = &["p{color:rgb(0,0,0)}p:last-child{color:rgb(255,0,0)}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p1 = dom.create_element(html("p"));
        dom.append_child(body, p1);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        assert!((color(&layout, p1)[0] - 1.0).abs() < 0.01, "p1 starts red (only/last child)");

        // Append p2: p1 is no longer :last-child, p2 is.
        let _ = drain(&mut dom);
        let p2 = dom.create_element(html("p"));
        dom.append_child(body, p2);
        let muts = drain(&mut dom);
        layout.apply(&dom, SHEET, &muts);

        // Oracle: full cascade of the mutated DOM.
        let mut oracle = StylePlane::new();
        run_cascade(&dom, &mut oracle, euclid::Size2D::new(W, H), SHEET, None);
        let oracle_color = |id| {
            *oracle.get(id).unwrap().borrow_data().unwrap()
                .styles.primary().get_inherited_text().color.into_srgb_legacy().raw_components()
        };

        assert_eq!(color(&layout, p1), oracle_color(p1), "p1 must match full re-cascade");
        assert_eq!(color(&layout, p2), oracle_color(p2), "p2 must match full re-cascade");
        assert!(color(&layout, p1)[0] < 0.01, "p1 recolored black (no longer last-child), got {:?}", color(&layout, p1));
        assert!((color(&layout, p2)[0] - 1.0).abs() < 0.01, "p2 is red (now last-child), got {:?}", color(&layout, p2));
    }

    /// `innerHTML` replace (a `SubtreeReplaced`) under a fixed-height `<body>`
    /// splices (the body's outer size is unchanged): the three new paragraphs land
    /// at the same absolute positions a full recompute produces. (Ported from the
    /// stateless `relayout_incremental` test it supersedes. The body is sized
    /// explicitly — its UA height is `auto`, which the replace would grow, taking
    /// the full-recompute fallback instead — and is `overflow: hidden` so it
    /// establishes a BFC: under the UA `p { margin }` a non-BFC body would take the
    /// margin-collapse fallback instead of splicing.)
    #[test]
    fn inner_html_replace_splices_matching_full() {
        const SHEET: &[&str] =
            &["html, body, p { display: block; } body { height: 200px; overflow: hidden; }"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p0 = dom.create_element(html("p"));
        dom.append_child(body, p0);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);
        dom.set_inner_html(body, "<p>one</p><p>two</p><p>three</p>");
        let muts = drain(&mut dom);
        assert_eq!(layout.apply(&dom, SHEET, &muts), Applied::Spliced);

        // Oracle: full cascade + layout of the mutated DOM.
        let mut oracle_styles = StylePlane::new();
        run_cascade(&dom, &mut oracle_styles, euclid::Size2D::new(W, H), SHEET, None);
        let oracle = lay_out(&dom, &oracle_styles, W, H);

        let kids: Vec<_> = dom.dom_children(body).collect();
        assert_eq!(kids.len(), 3, "body has the three replacement paragraphs");
        for &p in &kids {
            let c = oracle.rect_of(p).expect("oracle paragraph");
            let i = layout.fragments().rect_of(p).expect("spliced paragraph");
            assert!(
                (c.location.x - i.location.x).abs() < 0.5 && (c.location.y - i.location.y).abs() < 0.5,
                "paragraph abs pos: oracle=({},{}) spliced=({},{})",
                c.location.x, c.location.y, i.location.x, i.location.y,
            );
        }
    }

    // ── Orrery transform-motion perf spike (mere flip plan P0 / serval-as-host §8) ──
    //
    // §8's gate: does moving a node by its CSS transform land on the RepaintOnly
    // path (layout skipped), not full_relayout, at orrery scale?
    //
    // What these tests establish:
    //  • The relayout WORRY is unfounded — a transform value change is paint-tier
    //    (`RECALCULATE_OVERFLOW` < `RELAYOUT`) → `Applied::RepaintOnly`, no reflow,
    //    box geometry untouched, at N up to 1000. Proven on the CLASS path, which
    //    serval's incremental restyle handles today.
    //  • FINDING (tripwire): the orrery's *intended* mechanism — mutate each node's
    //    inline `style="transform:…"` every frame — is NOT yet picked up by the
    //    incremental restyle. A `style`-attribute change registers no damage
    //    (snapshot.rs marks it `other_attributes_changed`, which only drives
    //    `[attr]`-SELECTOR invalidation; inline-style re-cascade needs a
    //    `RESTYLE_STYLE_ATTRIBUTE` hint serval doesn't emit on the incremental
    //    path — the full `run_cascade` does apply it, dfe8702). So the gate's core
    //    fear is retired, but the orrery's continuous inline-transform motion has
    //    two serval prerequisites recorded in the flip plan P0.

    type NodeId = <ScriptedDom as LayoutDom>::NodeId;

    /// N `div.<classes>` nodes under `<body>`.
    fn build_nodes(n: usize, classes: &str) -> (ScriptedDom, Vec<NodeId>) {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let mut nodes = Vec::with_capacity(n);
        for _ in 0..n {
            let node = dom.create_element(html("div"));
            dom.set_attribute(node, attr("class"), classes);
            dom.append_child(body, node);
            nodes.push(node);
        }
        (dom, nodes)
    }

    /// `.t0..t4` differ ONLY in the translate value, so each class swap diffs to a
    /// transform-only change (`RECALCULATE_OVERFLOW`); `.n` fixes the box. The gate
    /// test swaps *forward* through distinct values (never back to a prior one):
    /// on the class path, returning to a previously-applied class value does not
    /// re-register damage (stylo class-based style sharing). That is a class-path
    /// artifact, not the §8 question — the orrery's motion path is inline-style
    /// (see the tripwire test) — so the gate uses fresh values each frame.
    const T_SHEET: &[&str] = &[
        ".n{position:absolute;width:80px;height:40px}",
        ".t0{transform:translate(10px,10px)}",
        ".t1{transform:translate(40px,40px)}",
        ".t2{transform:translate(70px,20px)}",
        ".t3{transform:translate(25px,90px)}",
        ".t4{transform:translate(55px,5px)}",
    ];

    /// THE GATE (relayout classification — the §8 core question), now exercised
    /// across multiple frames. A transform value change is paint-tier: each frame
    /// `apply()` returns `RepaintOnly` (layout skipped), the damage is
    /// `RECALCULATE_OVERFLOW` without `RELAYOUT`, and box geometry is untouched —
    /// at orrery scale (N up to 1000). The multi-frame sweep also proves repeated
    /// incremental applies re-register (prereq B). Forward fresh values each frame
    /// (t1→t2→t3→t4), never back to a prior class (class-based style sharing
    /// suppresses a repeated value — a class-path artifact, not the orrery's
    /// inline-style path; see `inline_style_transform_restyles_repaint_only`).
    #[test]
    fn transform_change_is_repaint_only_not_relayout() {
        for n in [200usize, 1000] {
            let (mut dom, nodes) = build_nodes(n, "n t0");
            let mut layout = IncrementalLayout::new(&dom, T_SHEET, W, H);
            let sizes0: Vec<_> =
                nodes.iter().map(|&node| layout.fragments().rect_of(node).unwrap().size).collect();
            let _ = drain(&mut dom);

            for cls in ["n t1", "n t2", "n t3", "n t4"] {
                for &node in &nodes {
                    dom.set_attribute(node, attr("class"), cls);
                }
                let muts = drain(&mut dom);
                let applied = layout.apply(&dom, T_SHEET, &muts);
                assert_eq!(applied, Applied::RepaintOnly, "N={n} {cls}: transform change must skip layout");
                assert!(
                    layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW),
                    "N={n} {cls}: transform must register paint-tier damage",
                );
                assert!(
                    !layout.last_damage().contains(RestyleDamage::RELAYOUT),
                    "N={n} {cls}: transform must NOT force relayout",
                );
            }
            for (&node, size0) in nodes.iter().zip(&sizes0) {
                let now = layout.fragments().rect_of(node).unwrap().size;
                assert!(
                    (now.width - size0.width).abs() < 0.5 && (now.height - size0.height).abs() < 0.5,
                    "N={n}: transform must not resize the box",
                );
            }
        }
    }

    /// Prereq B (fixed): repeated incremental `apply()` re-registers each change.
    /// Each sequential `RepaintOnly` apply resets `handled_snapshot` so Stylo's
    /// invalidator consumes that pass's snapshot — so a second (and third)
    /// transform change each register paint-tier damage, not just the first.
    /// Continuous per-frame motion via repeated apply now works. (Fresh forward
    /// values t1→t2→t3 — never back to a prior class.)
    #[test]
    fn sequential_repaint_only_applies_each_re_register() {
        let (mut dom, nodes) = build_nodes(4, "n t0");
        let mut layout = IncrementalLayout::new(&dom, T_SHEET, W, H);
        let _ = drain(&mut dom);

        for cls in ["n t1", "n t2", "n t3"] {
            for &node in &nodes {
                dom.set_attribute(node, attr("class"), cls);
            }
            let muts = drain(&mut dom);
            let applied = layout.apply(&dom, T_SHEET, &muts);
            assert_eq!(applied, Applied::RepaintOnly, "{cls}: paint-tier, skip layout");
            assert!(
                layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW),
                "{cls}: each sequential transform change must re-register (prereq B)",
            );
        }
    }

    /// CONTROL: a width change (also class-driven) IS layout-affecting → relayouts.
    /// Proves the harness can see the bad case (false-negative guard for the gate).
    #[test]
    fn width_change_relayouts_control() {
        const SHEET: &[&str] =
            &[".n{position:absolute;height:40px}.w0{width:50px}.w1{width:200px}"];
        let (mut dom, nodes) = build_nodes(8, "n w0");
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);

        for &node in &nodes {
            dom.set_attribute(node, attr("class"), "n w1");
        }
        let muts = drain(&mut dom);
        let applied = layout.apply(&dom, SHEET, &muts);
        assert_eq!(applied, Applied::Restyled, "width change must relayout (harness sees the bad case)");
        assert!(layout.last_damage().contains(RestyleDamage::RELAYOUT), "width must register RELAYOUT");
    }

    /// Prereq A (fixed): the orrery's mechanism — mutate each node's inline
    /// `style="transform:translate(x,y)"` — IS picked up by the incremental restyle
    /// AND is paint-tier. The `style`-attribute change sets `RESTYLE_STYLE_ATTRIBUTE`,
    /// the cascade re-applies the inline declaration block under the plane's stable
    /// lock, and each per-frame value→value change is `RepaintOnly` +
    /// `RECALCULATE_OVERFLOW`, no `RELAYOUT`. Multiple sequential inline-style frames
    /// each re-register (prereq B holds on the inline-style path too). The node
    /// starts transform-bearing (the materialization rule, next test).
    #[test]
    fn inline_style_transform_restyles_repaint_only() {
        const SHEET: &[&str] = &[".n{position:absolute;width:80px;height:40px}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let node = dom.create_element(html("div"));
        dom.set_attribute(node, attr("class"), "n");
        dom.set_attribute(node, attr("style"), "transform:translate(1px,1px)"); // start transform-bearing
        dom.append_child(body, node);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);

        for (x, y) in [(40, 40), (90, 15), (5, 70)] {
            dom.set_attribute(node, attr("style"), &format!("transform:translate({x}px,{y}px)"));
            let muts = drain(&mut dom);
            let applied = layout.apply(&dom, SHEET, &muts);
            assert_eq!(applied, Applied::RepaintOnly, "inline transform value→value is paint-tier → skip layout");
            assert!(
                layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW),
                "inline transform must register paint-tier damage (prereq A)",
            );
            assert!(
                !layout.last_damage().contains(RestyleDamage::RELAYOUT),
                "inline transform must NOT force relayout",
            );
        }
    }

    /// The orrery materialization rule, documented. A node GAINING a transform
    /// (none→value) relayouts once — gaining a transform establishes a containing
    /// block / stacking context, so Stylo conservatively reflows (correct, not a
    /// bug); value→value thereafter is `RepaintOnly`. So `cull_aabb` must
    /// materialize nodes already transform-bearing, so a node's first *moved* frame
    /// is value→value, never a relayout.
    #[test]
    fn inline_transform_first_application_relayouts_then_repaints() {
        const SHEET: &[&str] = &[".n{position:absolute;width:80px;height:40px}"];
        let (mut dom, nodes) = build_nodes(1, "n"); // no initial transform
        let node = nodes[0];
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);

        dom.set_attribute(node, attr("style"), "transform:translate(10px,10px)");
        let muts = drain(&mut dom);
        assert_eq!(layout.apply(&dom, SHEET, &muts), Applied::Restyled, "gaining a transform relayouts once");

        dom.set_attribute(node, attr("style"), "transform:translate(20px,30px)");
        let muts = drain(&mut dom);
        assert_eq!(
            layout.apply(&dom, SHEET, &muts), Applied::RepaintOnly,
            "subsequent transform changes skip layout",
        );
    }

    /// Sustained orrery-style motion over a long session — the regression guard
    /// for the **persistent Stylist**. Each inline-`style` transform change takes
    /// the cheap `RESTYLE_STYLE_ATTRIBUTE` replacement path, which reuses the prior
    /// frame's rule node held on `ElementData`; that is sound only because the
    /// rule tree persists across frames. The replacement also drops the prior
    /// style-attribute rule node onto the tree's free list (~1/frame); `maybe_gc`
    /// reclaims them once past Stylo's `RULE_TREE_GC_INTERVAL` (300). Driving 400
    /// frames crosses that threshold, so this exercises both the persistent-tree
    /// reuse AND the GC. Each frame must stay `RepaintOnly` + `RECALCULATE_OVERFLOW`
    /// with stable box geometry. A fresh-Stylist-per-pass would make the reused
    /// node dangle (the use-after-free the persistent design fixed); a GC bug would
    /// corrupt or crash here.
    #[test]
    fn sustained_inline_transform_motion_stays_repaint_only() {
        const SHEET: &[&str] = &[".n{position:absolute;width:80px;height:40px}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let node = dom.create_element(html("div"));
        dom.set_attribute(node, attr("class"), "n");
        // Materialized transform-bearing (so every frame is value→value, never the
        // none→value relayout — the orrery materialization rule).
        dom.set_attribute(node, attr("style"), "transform:translate(0px,0px)");
        dom.append_child(body, node);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let size0 = layout.fragments().rect_of(node).unwrap().size;
        let _ = drain(&mut dom);

        for i in 1..=400u32 {
            // Distinct from the prior frame each time (x advances by 1 mod 100).
            let (x, y) = (i % 100, (i * 3) % 100);
            dom.set_attribute(node, attr("style"), &format!("transform:translate({x}px,{y}px)"));
            let muts = drain(&mut dom);
            let applied = layout.apply(&dom, SHEET, &muts);
            assert_eq!(applied, Applied::RepaintOnly, "frame {i}: sustained inline transform must stay paint-tier");
            assert!(
                layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW)
                    && !layout.last_damage().contains(RestyleDamage::RELAYOUT),
                "frame {i}: transform-only damage, no relayout",
            );
        }
        let now = layout.fragments().rect_of(node).unwrap().size;
        assert!(
            (now.width - size0.width).abs() < 0.5 && (now.height - size0.height).abs() < 0.5,
            "sustained motion must never resize the box",
        );
    }

    /// A5 — the session owns its document viewport: a tall page reports a scroll
    /// range, an over-scroll clamps to it, and `emit_paint_list` paints the
    /// document at the clamped offset (the `-range` translate wrap).
    #[test]
    fn session_viewport_scroll_clamps_to_range_and_paints_scrolled() {
        use paint_list_api::{PaintCmd, PaintList};

        const SHEET: &[&str] = &["html,body,div{display:block;margin:0}.tall{height:2000px}"];
        let (dom, _) = build_nodes(1, "tall");
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);

        // 2000px of content in a 600px viewport → 1400px of vertical scroll.
        let range = layout.scroll_range(&dom);
        assert!((range.1 - 1400.0).abs() < 1.0, "range.y = content(2000) - viewport(600): {}", range.1);

        // An over-scroll clamps to the range.
        layout.set_viewport_scroll(&dom, (0.0, 5000.0));
        assert!(
            (layout.viewport_scroll().1 - 1400.0).abs() < 1.0,
            "over-scroll clamps to the range: {:?}",
            layout.viewport_scroll(),
        );

        // The session emits the document at the clamped scroll: a -1400 translate.
        let scroll = ScrollOffsets::default();
        let dev = DeviceIntSize::new(W as i32, H as i32);
        let pl = layout.emit_paint_list(&dom, &scroll, dev);
        assert!(
            pl.commands().iter().any(|c| matches!(c, PaintCmd::PushTransform(t)
                if t.origin.x.abs() < 0.5 && (t.origin.y + 1400.0).abs() < 0.5)),
            "emit carries the document scroll as a -1400 translate wrap",
        );
    }

    /// A5 — `overflow: hidden` on the root propagates to the viewport and disables
    /// document scroll (the session pins it at 0), even when the content overflows.
    #[test]
    fn session_viewport_scroll_respects_root_overflow_hidden() {
        const SHEET: &[&str] =
            &["html,body,div{display:block;margin:0}html{overflow:hidden}.tall{height:2000px}"];
        let (dom, _) = build_nodes(1, "tall");
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);

        layout.set_viewport_scroll(&dom, (0.0, 500.0));
        assert_eq!(
            layout.viewport_scroll(),
            (0.0, 0.0),
            "overflow:hidden on the root pins the viewport (no document scroll)",
        );
    }

    /// V2 — keyboard scroll defaults (`scroll_for_key`): an arrow steps a line,
    /// `PageDown` a viewport less one line, `Home`/`End` jump to the range ends, all
    /// clamped (an edge is a no-op).
    #[test]
    fn scroll_for_key_steps_lines_pages_and_ends() {
        const SHEET: &[&str] = &["html,body,div{display:block;margin:0}.tall{height:2000px}"];
        let (dom, _) = build_nodes(1, "tall");
        // W=800, H=600 → range.y = content(2000) - viewport(600) = 1400.
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);

        // Arrow down steps one line (40px).
        assert!(layout.scroll_for_key(&dom, ScrollKey::Down));
        assert!(
            (layout.viewport_scroll().1 - 40.0).abs() < 0.5,
            "arrow = 40px: {:?}",
            layout.viewport_scroll(),
        );

        // PageDown steps a viewport less a line (600 - 40 = 560): 40 → 600.
        assert!(layout.scroll_for_key(&dom, ScrollKey::PageDown));
        assert!(
            (layout.viewport_scroll().1 - 600.0).abs() < 0.5,
            "page = +560: {:?}",
            layout.viewport_scroll(),
        );

        // End jumps to the bottom of the range (1400); another PageDown is a no-op.
        assert!(layout.scroll_for_key(&dom, ScrollKey::End));
        assert!(
            (layout.viewport_scroll().1 - 1400.0).abs() < 1.0,
            "End = bottom 1400: {:?}",
            layout.viewport_scroll(),
        );
        assert!(!layout.scroll_for_key(&dom, ScrollKey::PageDown), "no movement past the bottom");

        // Home jumps back to the top; another arrow up is a no-op.
        assert!(layout.scroll_for_key(&dom, ScrollKey::Home));
        assert_eq!(layout.viewport_scroll(), (0.0, 0.0), "Home = top");
        assert!(!layout.scroll_for_key(&dom, ScrollKey::Up), "no movement above the top");
    }

    /// V2 — `scroll_to_element` brings an element's top to the viewport top
    /// (block-start scroll-into-view), clamped: a target 1000px down scrolls the
    /// document to y=1000.
    #[test]
    fn scroll_to_element_aligns_its_top_to_the_viewport() {
        const SHEET: &[&str] =
            &["html,body,div{display:block;margin:0}.tall{height:1000px}.target{height:50px}"];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let top = dom.create_element(html("div"));
        dom.set_attribute(top, attr("class"), "tall");
        dom.append_child(body, top);
        let target = dom.create_element(html("div"));
        dom.set_attribute(target, attr("class"), "target");
        dom.append_child(body, target);
        let bottom = dom.create_element(html("div"));
        dom.set_attribute(bottom, attr("class"), "tall");
        dom.append_child(body, bottom);

        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        // Content = 1000 + 50 + 1000 = 2050; range.y = 2050 - 600 = 1450. The target
        // top sits at y=1000 (after the first tall spacer), within range.
        assert!(layout.scroll_to_element(&dom, target));
        assert!(
            (layout.viewport_scroll().1 - 1000.0).abs() < 1.0,
            "the target's top is brought to the viewport top: {:?}",
            layout.viewport_scroll(),
        );
        // Scrolling to it again is a no-op (already in position).
        assert!(!layout.scroll_to_element(&dom, target), "already in position");
    }

    /// `scroll_element_into_view` uses "nearest" alignment (focus / Tab), the minimum
    /// scroll to make the element visible: a target below the fold brings its *bottom*
    /// to the viewport bottom (not its top to the top, as anchor navigation would),
    /// an already-visible target is a no-op, and a target above the current scroll
    /// brings its top down to the viewport top.
    #[test]
    fn scroll_element_into_view_uses_nearest_alignment() {
        const SHEET: &[&str] = &[
            "html,body,div{display:block;margin:0}.spacer{height:1000px}.target{height:50px}",
        ];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let top = dom.create_element(html("div"));
        dom.set_attribute(top, attr("class"), "spacer");
        dom.append_child(body, top);
        let target = dom.create_element(html("div"));
        dom.set_attribute(target, attr("class"), "target");
        dom.append_child(body, target);
        let bottom = dom.create_element(html("div"));
        dom.set_attribute(bottom, attr("class"), "spacer");
        dom.append_child(body, bottom);

        // W=800, H=600. The target box is y=1000..1050. Content = 2050; range.y = 1450.
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);

        // From the top, the target (1000..1050) is below the 600px fold. "nearest"
        // brings its bottom (1050) to the viewport bottom: scroll = 1050 - 600 = 450
        // (anchor "start" alignment would scroll to 1000 instead).
        assert!(layout.scroll_element_into_view(&dom, target));
        assert!(
            (layout.viewport_scroll().1 - 450.0).abs() < 1.0,
            "nearest brings the bottom to the fold (450), not the top to the top (1000): {:?}",
            layout.viewport_scroll(),
        );
        // Now fully visible (1000..1050 within 450..1050) → no-op.
        assert!(!layout.scroll_element_into_view(&dom, target), "already visible: no jump");

        // Scroll past it, then bring it back: now above the window, so its top (1000)
        // comes to the viewport top.
        layout.set_viewport_scroll(&dom, (0.0, 1400.0));
        assert!(layout.scroll_element_into_view(&dom, target));
        assert!(
            (layout.viewport_scroll().1 - 1000.0).abs() < 1.0,
            "off the top brings the top edge to the viewport top (1000): {:?}",
            layout.viewport_scroll(),
        );
    }

    /// Build `body > div.scroller > (div.top, div.bottom)` — a 100×100
    /// `overflow:scroll` container over 500px of stacked content (two 250px
    /// blocks). Returns the dom plus the scroller / top / bottom ids.
    fn build_nested_scroller() -> (ScriptedDom, NodeId, NodeId, NodeId) {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let scroller = dom.create_element(html("div"));
        dom.set_attribute(scroller, attr("class"), "scroller");
        dom.append_child(body, scroller);
        let top = dom.create_element(html("div"));
        dom.set_attribute(top, attr("class"), "top");
        dom.append_child(scroller, top);
        let bottom = dom.create_element(html("div"));
        dom.set_attribute(bottom, attr("class"), "bottom");
        dom.append_child(scroller, bottom);
        (dom, scroller, top, bottom)
    }

    const NESTED_SCROLL_SHEET: &[&str] = &[
        "html,body,div{display:block;margin:0;padding:0;border:0} \
         .scroller{overflow:scroll;width:100px;height:100px} \
         .top{height:250px} .bottom{height:250px}",
    ];

    /// Nested element scrolling: `scroll_at` over an `overflow:scroll` container
    /// routes the wheel delta into *that* container (not the document), clamps to its
    /// scrollable extent, and the change is observable through the session's own
    /// hit-test (a fixed scene point resolves to deeper content as the container
    /// scrolls under it — the merge into `hit_test` working end to end).
    #[test]
    fn nested_overflow_scroll_routes_to_container_and_clamps() {
        let (dom, _scroller, top, bottom) = build_nested_scroller();
        let mut layout = IncrementalLayout::new(&dom, NESTED_SCROLL_SHEET, W, H);
        let scroll = ScrollOffsets::default();

        // Unscrolled: scene (50,50) is over the first 250px block.
        assert_eq!(layout.hit_test(&dom, 50.0, 50.0, &scroll), Some(top), "starts over .top");

        // Wheel down 300px inside the scroller → routes to the container (true), and
        // the same scene point now resolves to the second block scrolled under it.
        assert!(layout.scroll_at(&dom, 50.0, 50.0, 0.0, 300.0), "the scroller consumes the delta");
        assert_eq!(
            layout.hit_test(&dom, 50.0, 50.0, &scroll),
            Some(bottom),
            "scrolled 300px, the point now resolves to .bottom (content moved up under it)",
        );

        // Content 500px in a 100px scrollport → max scroll 400px. An over-scroll past
        // it still moves (clamps to 400, still .bottom), but a further wheel at the
        // limit is a no-op: the container is pinned and the document does not scroll
        // (it fits), so `scroll_at` returns false (chaining found no taker).
        assert!(layout.scroll_at(&dom, 50.0, 50.0, 0.0, 1000.0), "over-scroll clamps but still moves to the limit");
        assert_eq!(layout.hit_test(&dom, 50.0, 50.0, &scroll), Some(bottom), "still over .bottom at the limit");
        assert!(
            !layout.scroll_at(&dom, 50.0, 50.0, 0.0, 1000.0),
            "at the scroll limit with a non-scrolling document, the wheel is a no-op",
        );
    }

    /// The nested scroll offset reaches paint: after `scroll_at`, `emit_paint_list`
    /// carries the container's content translated by `-offset` (the scroll wrap the
    /// renderer composites under the clip). The merge into `emit_paint_list` working.
    #[test]
    fn nested_scroll_translates_the_emitted_paint() {
        use paint_list_api::{PaintCmd, PaintList};

        let (dom, _scroller, _top, _bottom) = build_nested_scroller();
        let mut layout = IncrementalLayout::new(&dom, NESTED_SCROLL_SHEET, W, H);
        let scroll = ScrollOffsets::default();
        let dev = DeviceIntSize::new(W as i32, H as i32);

        // Before scrolling, no -150 translate exists.
        let before = layout.emit_paint_list(&dom, &scroll, dev);
        assert!(
            !before.commands().iter().any(|c| matches!(c, PaintCmd::PushTransform(t)
                if (t.origin.y + 150.0).abs() < 0.5)),
            "unscrolled: no scroll translate yet",
        );

        assert!(layout.scroll_at(&dom, 50.0, 50.0, 0.0, 150.0), "scroll the container 150px");

        // After: the scroller's content is translated by (0, -150).
        let after = layout.emit_paint_list(&dom, &scroll, dev);
        assert!(
            after.commands().iter().any(|c| matches!(c, PaintCmd::PushTransform(t)
                if t.origin.x.abs() < 0.5 && (t.origin.y + 150.0).abs() < 0.5)),
            "the nested scroll paints the content at a -150 translate wrap",
        );
    }

    /// Scroll chaining's base case: a point with no `overflow:scroll/auto` ancestor
    /// falls through to the document viewport, so `scroll_at` over a plain tall page
    /// scrolls the document (the same path `scroll_by` drives), not a nested map.
    #[test]
    fn scroll_at_falls_through_to_the_document_viewport() {
        const SHEET: &[&str] = &["html,body,div{display:block;margin:0}.tall{height:2000px}"];
        let (dom, _) = build_nodes(1, "tall");
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);

        // No scroll container under the point → the document viewport takes the delta.
        assert!(layout.scroll_at(&dom, 50.0, 50.0, 0.0, 40.0), "the document consumes the wheel");
        assert!(
            (layout.viewport_scroll().1 - 40.0).abs() < 0.5,
            "the document scrolled 40px: {:?}",
            layout.viewport_scroll(),
        );
    }

    /// Inline links are hit-testable (the `elementFromPoint` descent): a click on an
    /// inline `<a>`'s text resolves to the `<a>`, while a click past its text (the
    /// line's empty trailing space) resolves to the containing block — containment,
    /// not a line-wide rect. (The bug that made inline links unclickable.)
    #[test]
    fn inline_link_is_hit_testable() {
        // Ahem renders each glyph as a solid em square, so "AAAA" at 20px is an exact
        // 80×20 box at the paragraph's content origin (margins / padding / border 0).
        const SHEET: &[&str] = &[
            "html,body,p{margin:0;padding:0;border:0} p{font-family:Ahem;font-size:20px} a{color:rgb(0,0,255)}",
        ];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);
        let a = dom.create_element(html("a"));
        dom.set_attribute(a, attr("href"), "/dest");
        dom.append_child(p, a);
        let t = dom.create_text("AAAA");
        dom.append_child(a, t);

        let layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let scroll = ScrollOffsets::default();
        // On the link text (the 80×20 "AAAA" box): resolves to the inline <a>.
        assert_eq!(
            layout.hit_test(&dom, 30.0, 10.0, &scroll),
            Some(a),
            "a click on the link text resolves to the inline <a>",
        );
        // Past the link text on the same line (x=200, beyond the 80px run): the empty
        // trailing space is not the link — resolves to the containing <p>.
        let off = layout.hit_test(&dom, 200.0, 10.0, &scroll);
        assert_ne!(off, Some(a), "a click past the link text must not hit the <a>");
        assert_eq!(off, Some(p), "...it resolves to the containing block <p>");
    }

    /// A wrapped inline `<a>` is a *set* of per-line run rects, not a union bounding
    /// box: a point on the short second line, past its text but within the longer
    /// first line's x-extent, must NOT hit the anchor (a union rect would wrongly
    /// claim it). Guards the multi-line conformance pitfall the standards review
    /// flagged (CSS2.2 §9.4.2: an inline box split across lines is several boxes).
    #[test]
    fn wrapped_inline_link_uses_per_line_rects_not_a_union() {
        // 85px box, 20px Ahem: "AAAA" (80px) fills line 1; the space breaks and "BB"
        // (40px) drops to line 2. A union x-extent would be 0..80 over both lines; the
        // set is 0..80 on line 1, 0..40 on line 2.
        const SHEET: &[&str] = &[
            "html,body,p{margin:0;padding:0;border:0} p{width:85px;font-family:Ahem;font-size:20px} a{color:rgb(0,0,255)}",
        ];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);
        let a = dom.create_element(html("a"));
        dom.set_attribute(a, attr("href"), "/dest");
        dom.append_child(p, a);
        let t = dom.create_text("AAAA BB");
        dom.append_child(a, t);

        let layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let scroll = ScrollOffsets::default();
        // Line 1 "AAAA" (y~10, x 0..80) and line 2 "BB" (y~30, x 0..40) are the anchor.
        assert_eq!(layout.hit_test(&dom, 60.0, 10.0, &scroll), Some(a), "line 1, on AAAA");
        assert_eq!(layout.hit_test(&dom, 20.0, 30.0, &scroll), Some(a), "line 2, on BB");
        // Line 2, x=60: past "BB" (0..40) but within line 1's "AAAA" x-extent. The set
        // does not hit; a union rect (0..80 × 0..40) would. Must resolve to <p>.
        let gutter = layout.hit_test(&dom, 60.0, 30.0, &scroll);
        assert_ne!(gutter, Some(a), "a union rect would false-hit here; the set must not");
        assert_eq!(gutter, Some(p), "...it resolves to the containing block <p>");
    }

    /// A colour-only inline `<a>` mid-paragraph is hit-testable. parley does not
    /// split runs on colour (it is a per-cluster `Brush`, not a shaping boundary), so
    /// the link shapes into the *same* glyph run as the surrounding text. Resolving
    /// by the run's first byte would attribute the whole run to the text before the
    /// link, making it unhittable (the diagnosed bug); cluster-granularity resolution
    /// maps each glyph's own byte, so a click on the link's glyphs resolves to the
    /// `<a>` and a click on the surrounding text to the block `<p>`.
    #[test]
    fn colour_only_inline_link_is_hit_testable() {
        // Ahem 20px: every glyph is a 20px em square. "XXLINKYY" lays out on one line,
        // bytes 0..1 "XX", 2..5 "LINK" (the <a>), 6..7 "YY"; x = 20*index. The <a>
        // differs from its siblings ONLY in colour, so all eight glyphs share one run.
        const SHEET: &[&str] = &[
            "html,body,p{margin:0;padding:0;border:0} p{font-family:Ahem;font-size:20px} a{color:rgb(0,0,255)}",
        ];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let p = dom.create_element(html("p"));
        dom.append_child(body, p);
        let before = dom.create_text("XX");
        dom.append_child(p, before);
        let a = dom.create_element(html("a"));
        dom.set_attribute(a, attr("href"), "/dest");
        dom.append_child(p, a);
        let link_text = dom.create_text("LINK");
        dom.append_child(a, link_text);
        let after = dom.create_text("YY");
        dom.append_child(p, after);

        let layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let scroll = ScrollOffsets::default();
        // On the link glyphs (x 40..120, e.g. x=60 over the 4th glyph): resolves to <a>
        // even though the run starts in the preceding "XX" text.
        assert_eq!(
            layout.hit_test(&dom, 60.0, 10.0, &scroll),
            Some(a),
            "a click on the colour-only link's glyphs resolves to the inline <a>",
        );
        // On the text before / after the link (same run): resolves to the block <p>,
        // not the link — the cluster's byte is outside the <a>'s source range.
        assert_eq!(
            layout.hit_test(&dom, 10.0, 10.0, &scroll),
            Some(p),
            "the text before the link resolves to the containing <p>, not the <a>",
        );
        assert_eq!(
            layout.hit_test(&dom, 130.0, 10.0, &scroll),
            Some(p),
            "the text after the link resolves to the containing <p>, not the <a>",
        );
    }

    /// `pointer-events: none` removes a box as a hit target so the point falls through
    /// to what is behind it, but a `pointer-events: auto` descendant inside it stays
    /// hittable (the CSS-UI non-blanket rule, which the inherited computed value
    /// encodes for free).
    #[test]
    fn pointer_events_none_falls_through_but_auto_descendant_still_hits() {
        const SHEET: &[&str] = &[
            "html,body{margin:0} \
             .target{position:absolute;left:0;top:0;width:100px;height:100px} \
             .overlay{position:absolute;left:0;top:0;width:100px;height:100px;pointer-events:none} \
             .live{position:absolute;left:0;top:0;width:50px;height:50px;pointer-events:auto}",
        ];
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let h = dom.create_element(html("html"));
        dom.append_child(root, h);
        let body = dom.create_element(html("body"));
        dom.append_child(h, body);
        let target = dom.create_element(html("div"));
        dom.set_attribute(target, attr("class"), "target");
        dom.append_child(body, target);
        // The overlay is later in DOM (paints on top) and is pointer-events:none; it
        // carries a small pointer-events:auto patch.
        let overlay = dom.create_element(html("div"));
        dom.set_attribute(overlay, attr("class"), "overlay");
        dom.append_child(body, overlay);
        let live = dom.create_element(html("div"));
        dom.set_attribute(live, attr("class"), "live");
        dom.append_child(overlay, live);

        let layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let scroll = ScrollOffsets::default();
        // Over the overlay but outside the live patch: the none overlay falls through
        // to the target beneath it (without the rule it would hit the topmost overlay).
        assert_eq!(
            layout.hit_test(&dom, 75.0, 75.0, &scroll),
            Some(target),
            "a pointer-events:none box falls through to what is behind it",
        );
        // Over the live patch (auto, inside the none overlay): it stays hittable.
        assert_eq!(
            layout.hit_test(&dom, 25.0, 25.0, &scroll),
            Some(live),
            "a pointer-events:auto descendant of a none box still hits",
        );
    }
}
