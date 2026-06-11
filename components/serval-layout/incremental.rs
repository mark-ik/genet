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
use crate::paint_emit::{emit_paint_list_with_layouts, ScrollOffsets, ServalPaintList};
use crate::serval_lane::ServalLaneView;
use crate::style::StylePlane;
use crate::subtree::SubtreeView;
use crate::text_measure::TextMeasureCtx;

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
    /// Whether `built` / `text_ctx` still match `fragments`. Set by every full
    /// layout; cleared by a structural splice (which updates `fragments` but not
    /// the box-tree side-table). [`emit_paint_list`](Self::emit_paint_list)
    /// requires it.
    paint_side_valid: bool,
    width: f32,
    height: f32,
    /// Aggregate `RestyleDamage` from the most recent attribute-only
    /// [`apply`](Self::apply). Lets callers/tests confirm which paint-tier bits
    /// a batch produced (e.g. a transform-only motion frame registers
    /// `RECALCULATE_OVERFLOW` without `RELAYOUT`). `empty()` before any restyle.
    last_damage: RestyleDamage,
}

impl<Id: Copy + Eq + Hash + 'static> IncrementalLayout<Id> {
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
        Self {
            styles,
            stylist,
            sheets: stylesheets.iter().map(|s| s.to_string()).collect(),
            fragments,
            built,
            text_ctx,
            paint_side_valid: true,
            width,
            height,
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
    /// scroll-aware via `scroll`. `None` if the point falls outside every fragment.
    pub fn hit_test<D>(&self, dom: &D, x: f32, y: f32, scroll: &ScrollOffsets<Id>) -> Option<Id>
    where
        D: LayoutDom<NodeId = Id>,
    {
        let view =
            ServalLaneView::new(dom, &self.styles, &self.fragments).with_scroll_offsets(scroll);
        let hit = view.hit_test(Point::new(x, y))?;
        view.find_by_source_id(hit.source_node)
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
            Applied::Restyled
        } else {
            // Paint-only: prior fragments (and box-tree side-table) still valid.
            Applied::RepaintOnly
        }
    }

    /// Emit a glyph-bearing [`ServalPaintList`] from the current layout — the
    /// engine-agnostic command stream a host composites or lowers to a scene.
    /// Valid on the `RepaintOnly` path (a transform-only frame keeps box
    /// geometry, so the retained box tree + text context still match the
    /// fragments). Empty image planes, matching the scripted layout path.
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
        let images = ImagePlane::new();
        let bg_images = BackgroundImagePlane::new();
        emit_paint_list_with_layouts(
            dom,
            &self.styles,
            &self.fragments,
            &self.built,
            &self.text_ctx,
            &images,
            &bg_images,
            scroll_offsets,
            viewport,
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
            // Outer size change → ancestors would reflow → full fallback.
            if (scoped_root.size.width - prior_root.size.width).abs() >= 0.5
                || (scoped_root.size.height - prior_root.size.height).abs() >= 0.5
            {
                return self.full_relayout(dom);
            }
            // Splice: translate the scoped subtree to its real position.
            let dx = prior_root.location.x - scoped_root.location.x;
            let dy = prior_root.location.y - scoped_root.location.y;
            let mut subtree = Vec::new();
            collect_subtree(dom, root, &mut subtree);
            for node in subtree {
                if let Some(layout) = scoped.rect_of(node) {
                    let mut translated = *layout;
                    translated.location.x += dx;
                    translated.location.y += dy;
                    result.insert(node, translated);
                }
            }
        }
        self.fragments = result;
        // The splice updates fragments but not the box-tree side-table, so a
        // following emit_paint_list would mismatch — mark it stale (a relayout
        // re-validates). Attribute-only hosts (the pool) never take this path.
        self.paint_side_valid = false;
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

/// Lay out over an already-cascaded plane (no images in the scripted
/// path), hiding the taffy viewport type.
fn lay_out<D>(dom: &D, styles: &StylePlane<D::NodeId>, width: f32, height: f32) -> FragmentPlane<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
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
    D::NodeId: Copy + Eq + Hash,
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
    /// incrementally (`Spliced`): appending a `<p>` under the full-height
    /// `<body>` (UA `height:100%`) re-lays-out the body subtree, and the
    /// new `<p>` lands where a full recompute would put it.
    #[test]
    fn structural_change_splices_incrementally() {
        const SHEET: &[&str] = &["p{height:20px}"];
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

    /// `innerHTML` replace (a `SubtreeReplaced`) under the full-height
    /// `<body>` splices: the three new paragraphs land at the same
    /// absolute positions a full recompute produces. (Ported from the
    /// stateless `relayout_incremental` test it supersedes.)
    #[test]
    fn inner_html_replace_splices_matching_full() {
        const SHEET: &[&str] = &["html, body, p { display: block; }"];
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
}
