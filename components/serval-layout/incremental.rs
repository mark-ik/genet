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

use crate::cascade::{restyle_with_snapshots, run_cascade};
use crate::fragment::FragmentPlane;
use crate::image_decode::ImagePlane;
use crate::style::StylePlane;

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
    /// Structural batch — full cascade + layout (the conservative path).
    FullRecompute,
}

/// A persistent cascade + layout session over one `LayoutDom`.
pub struct IncrementalLayout<Id: Copy + Eq + Hash> {
    styles: StylePlane<Id>,
    fragments: FragmentPlane<Id>,
    width: f32,
    height: f32,
}

impl<Id: Copy + Eq + Hash + 'static> IncrementalLayout<Id> {
    /// Initial full cascade + layout over `dom`.
    pub fn new<D>(dom: &D, stylesheets: &[&str], width: f32, height: f32) -> Self
    where
        D: LayoutDom<NodeId = Id>,
    {
        let mut styles = StylePlane::new();
        run_cascade(dom, &mut styles, euclid::Size2D::new(width, height), stylesheets);
        let fragments = lay_out(dom, &styles, width, height);
        Self { styles, fragments, width, height }
    }

    /// The current per-node fragment plane.
    pub fn fragments(&self) -> &FragmentPlane<Id> {
        &self.fragments
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
            // Structural change → full recompute (conservative + correct).
            let mut styles = StylePlane::new();
            run_cascade(dom, &mut styles, euclid::Size2D::new(self.width, self.height), stylesheets);
            self.fragments = lay_out(dom, &styles, self.width, self.height);
            self.styles = styles;
            return Applied::FullRecompute;
        }

        // Attribute-only → incremental restyle over the persistent plane.
        let outcome = restyle_with_snapshots(
            dom,
            &mut self.styles,
            euclid::Size2D::new(self.width, self.height),
            stylesheets,
            mutations,
        );
        if outcome.needs_relayout {
            self.fragments = lay_out(dom, &self.styles, self.width, self.height);
            Applied::Restyled
        } else {
            // Paint-only: prior fragments are still valid; skip layout.
            Applied::RepaintOnly
        }
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
        run_cascade(&dom, &mut oracle_styles, euclid::Size2D::new(W, H), SHEET);
        let oracle = lay_out(&dom, &oracle_styles, W, H);

        let inc = layout.fragments().rect_of(p).unwrap();
        let full = oracle.rect_of(p).unwrap();
        assert!((inc.size.width - full.size.width).abs() < 0.5, "width must match full layout");
        assert!((inc.size.width - 200.0).abs() < 0.5, "p should be 200px wide after restyle");
    }

    /// A structural change (append a child) takes the full-recompute path.
    #[test]
    fn structural_change_is_full_recompute() {
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
        assert_eq!(layout.apply(&dom, SHEET, &muts), Applied::FullRecompute);
        assert!(layout.fragments().rect_of(p).is_some(), "new <p> laid out after full recompute");
    }
}
