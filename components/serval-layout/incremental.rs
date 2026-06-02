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

use layout_dom_api::{DomMutation, LayoutDom};
use style::selector_parser::RestyleDamage;

use crate::cascade::{restyle_structural, restyle_with_snapshots, run_cascade};
use crate::fragment::FragmentPlane;
use crate::image_decode::ImagePlane;
use crate::invalidate::{classify, coalesce};
use crate::style::StylePlane;
use crate::subtree::SubtreeView;

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
    fragments: FragmentPlane<Id>,
    width: f32,
    height: f32,
    /// Aggregate `RestyleDamage` from the most recent attribute-only
    /// [`apply`](Self::apply). Lets callers/tests confirm which paint-tier bits
    /// a batch produced (e.g. a transform-only motion frame registers
    /// `RECALCULATE_OVERFLOW` without `RELAYOUT`). `empty()` before any restyle.
    last_damage: RestyleDamage,
}

impl<Id: Copy + Eq + Hash + 'static> IncrementalLayout<Id> {
    /// Initial full cascade + layout over `dom`.
    pub fn new<D>(dom: &D, stylesheets: &[&str], width: f32, height: f32) -> Self
    where
        D: LayoutDom<NodeId = Id>,
    {
        let mut styles = StylePlane::new();
        run_cascade(dom, &mut styles, euclid::Size2D::new(width, height), stylesheets, None);
        let fragments = lay_out(dom, &styles, width, height);
        Self { styles, fragments, width, height, last_damage: RestyleDamage::empty() }
    }

    /// The current per-node fragment plane.
    pub fn fragments(&self) -> &FragmentPlane<Id> {
        &self.fragments
    }

    /// The aggregate `RestyleDamage` from the most recent attribute-only
    /// [`apply`](Self::apply) (`empty()` before any, and unchanged by a
    /// structural batch, which takes the cascade-from-scratch path).
    pub fn last_damage(&self) -> RestyleDamage {
        self.last_damage
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

        let attribute_only = mutations
            .iter()
            .all(|m| matches!(m, DomMutation::AttributeChanged { .. }));

        if !attribute_only {
            return self.apply_structural(dom, stylesheets, mutations);
        }

        // Attribute-only → incremental restyle over the persistent plane.
        let outcome = restyle_with_snapshots(
            dom,
            &mut self.styles,
            euclid::Size2D::new(self.width, self.height),
            stylesheets,
            mutations,
        );
        self.last_damage = outcome.damage;
        if outcome.needs_relayout {
            self.fragments = lay_out(dom, &self.styles, self.width, self.height);
            Applied::Restyled
        } else {
            // Paint-only: prior fragments are still valid; skip layout.
            Applied::RepaintOnly
        }
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
        stylesheets: &[&str],
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
        restyle_structural(
            dom,
            &mut self.styles,
            euclid::Size2D::new(self.width, self.height),
            stylesheets,
            &root_ids,
        );

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
        Applied::Spliced
    }

    /// Full layout over the current (already-cascaded) styles. The
    /// fallback for the structural splice.
    fn full_relayout<D>(&mut self, dom: &D) -> Applied
    where
        D: LayoutDom<NodeId = Id>,
    {
        self.fragments = lay_out(dom, &self.styles, self.width, self.height);
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
    let images = ImagePlane::new();
    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width),
        height: taffy::AvailableSpace::Definite(height),
    };
    let (fragments, _tree, _ctx) = crate::layout::layout(dom, styles, &images, viewport);
    fragments
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

    /// THE GATE (relayout classification — the §8 core question). A transform
    /// value change is paint-tier: `apply()` returns `RepaintOnly` (layout
    /// skipped), the damage is `RECALCULATE_OVERFLOW` without `RELAYOUT`, and box
    /// geometry is untouched — at orrery scale (N up to 1000). So transform motion
    /// does NOT force reflow, retiring the central worry. (One change per node; a
    /// *sequence* of changes hits a separate incremental-restyle limitation — see
    /// `sequential_repaint_only_applies_drop_the_second_change`.)
    #[test]
    fn transform_change_is_repaint_only_not_relayout() {
        for n in [200usize, 1000] {
            let (mut dom, nodes) = build_nodes(n, "n t0");
            let mut layout = IncrementalLayout::new(&dom, T_SHEET, W, H);
            let sizes0: Vec<_> =
                nodes.iter().map(|&node| layout.fragments().rect_of(node).unwrap().size).collect();
            let _ = drain(&mut dom);

            for &node in &nodes {
                dom.set_attribute(node, attr("class"), "n t1"); // t0 → t1: a transform-only diff
            }
            let muts = drain(&mut dom);
            let applied = layout.apply(&dom, T_SHEET, &muts);
            assert_eq!(applied, Applied::RepaintOnly, "N={n}: transform change must skip layout");
            assert!(
                layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW),
                "N={n}: transform must register paint-tier damage",
            );
            assert!(
                !layout.last_damage().contains(RestyleDamage::RELAYOUT),
                "N={n}: transform must NOT force relayout",
            );
            for (&node, size0) in nodes.iter().zip(&sizes0) {
                let now = layout.fragments().rect_of(node).unwrap().size;
                assert!(
                    (now.width - size0.width).abs() < 0.5 && (now.height - size0.height).abs() < 0.5,
                    "N={n}: transform must not resize the box",
                );
            }
        }
    }

    /// FINDING / TRIPWIRE: a SECOND `apply()` after a `RepaintOnly` does not
    /// re-register the change — the first transform change is correctly
    /// `RepaintOnly` + `RECALCULATE_OVERFLOW`, but a subsequent transform change
    /// produces no paint-tier damage. So continuous per-frame motion via repeated
    /// `apply()` (the orrery's pattern) does not work on the current incremental
    /// path: the `RepaintOnly` layout-skip appears to leave stylo's restyle state
    /// uncleared for the next pass. A serval incremental-restyle prerequisite for
    /// the orrery, recorded in flip plan P0. When fixed, this assertion FLIPS.
    #[test]
    fn sequential_repaint_only_applies_drop_the_second_change() {
        let (mut dom, nodes) = build_nodes(4, "n t0");
        let mut layout = IncrementalLayout::new(&dom, T_SHEET, W, H);
        let _ = drain(&mut dom);

        // First transform change (t0 → t1): registers correctly.
        for &node in &nodes {
            dom.set_attribute(node, attr("class"), "n t1");
        }
        let muts = drain(&mut dom);
        assert_eq!(layout.apply(&dom, T_SHEET, &muts), Applied::RepaintOnly);
        assert!(
            layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW),
            "first transform change registers paint-tier damage",
        );

        // Second transform change (t1 → t2): currently dropped (the finding).
        for &node in &nodes {
            dom.set_attribute(node, attr("class"), "n t2");
        }
        let muts = drain(&mut dom);
        let applied = layout.apply(&dom, T_SHEET, &muts);
        assert_eq!(applied, Applied::RepaintOnly, "second apply still takes the attribute-only path");
        assert!(
            !layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW),
            "KNOWN GAP: the second sequential apply drops the change. If this now fails, \
             serval fixed repeated incremental restyle — a prerequisite for continuous motion",
        );
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

    /// FINDING / TRIPWIRE: the orrery's intended mechanism — mutate each node's
    /// inline `style="transform:translate(x,y)"` — is currently IGNORED by the
    /// incremental restyle. The `style`-attribute change registers no transform
    /// damage, so `apply()` returns `RepaintOnly` for a no-op reason (the change
    /// was never seen), not because a moved node cheaply repainted.
    ///
    /// When serval wires inline-style-attribute changes into the snapshot
    /// invalidator (a `RESTYLE_STYLE_ATTRIBUTE` hint), this assertion FLIPS — the
    /// signal that the orrery's continuous inline-transform motion is viable;
    /// update this test + flip plan P0/P1 then. (The full `run_cascade` DOES apply
    /// inline style — dfe8702 — so this is an incremental-path gap, not a parser gap.)
    #[test]
    fn inline_style_transform_is_ignored_by_incremental_restyle() {
        const SHEET: &[&str] = &[".n{position:absolute;width:80px;height:40px}"];
        let (mut dom, nodes) = build_nodes(1, "n");
        let node = nodes[0];
        let mut layout = IncrementalLayout::new(&dom, SHEET, W, H);
        let _ = drain(&mut dom);

        dom.set_attribute(node, attr("style"), "transform:translate(40px,40px)");
        let muts = drain(&mut dom);
        let applied = layout.apply(&dom, SHEET, &muts);

        assert_eq!(applied, Applied::RepaintOnly, "style-attr change takes the attribute-only path");
        assert!(
            !layout.last_damage().contains(RestyleDamage::RECALCULATE_OVERFLOW),
            "KNOWN GAP: inline-style transform is not picked up incrementally. If this \
             now fails, serval wired style-attr invalidation — update the orrery + flip plan",
        );
    }

    // (Per-frame timing at scale is deferred: it is premature until the
    // continuous-motion prerequisites above are met — repeated incremental
    // applies dropping subsequent changes, and inline-style not invalidating —
    // since a timing loop today would measure no-op restyles after the first.)
}
