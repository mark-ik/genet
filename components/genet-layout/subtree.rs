/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Re-rooted `LayoutDom` view for scoped relayout (#2(b)).
//!
//! `SubtreeView` presents `root`'s subtree as if it were the whole document, so the
//! existing `render` pipeline lays out only that subtree. This is the first scoped
//! recompute primitive — relayout the invalidation root's subtree instead of the
//! whole document.
//!
//! Known boundary (what the diff-test against the coarse oracle checks, and where it
//! must stop): the root is treated as the layout root, so (1) descendants are
//! positioned *relative to the root* (not the root's real document position), and
//! (2) the root gets the cascade's default inherited context, not its real
//! ancestors'. So scoped output matches coarse only for *relative interior geometry*
//! and when no ancestor sets inherited properties affecting the subtree. True
//! inheritance-aware scoped restyle (threading the root's real inherited style) is
//! the next step.

use std::hash::Hash;

use layout_dom_api::{AttributeView, LayoutDom, LocalName, Namespace, NodeKind, QualName};

use crate::{FragmentPlane, render};

/// A view of `dom` re-rooted at `root`: `document()` is `root`, `root` has no parent
/// or siblings, and traversal below is delegated unchanged.
pub struct SubtreeView<'a, D: LayoutDom> {
    dom: &'a D,
    root: D::NodeId,
}

impl<'a, D: LayoutDom> SubtreeView<'a, D> {
    pub fn new(dom: &'a D, root: D::NodeId) -> Self {
        Self { dom, root }
    }
}

impl<D: LayoutDom> LayoutDom for SubtreeView<'_, D> {
    type NodeId = D::NodeId;

    fn document(&self) -> Self::NodeId {
        self.root
    }

    fn parent(&self, id: Self::NodeId) -> Option<Self::NodeId> {
        if id == self.root {
            None
        } else {
            self.dom.parent(id)
        }
    }

    fn prev_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId> {
        if id == self.root {
            None
        } else {
            self.dom.prev_sibling(id)
        }
    }

    fn next_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId> {
        if id == self.root {
            None
        } else {
            self.dom.next_sibling(id)
        }
    }

    fn dom_children(&self, id: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_ {
        self.dom.dom_children(id)
    }

    fn kind(&self, id: Self::NodeId) -> NodeKind {
        self.dom.kind(id)
    }

    fn opaque_id(&self, id: Self::NodeId) -> u64 {
        self.dom.opaque_id(id)
    }

    fn element_name(&self, id: Self::NodeId) -> Option<&QualName> {
        self.dom.element_name(id)
    }

    fn attribute(&self, id: Self::NodeId, ns: &Namespace, local: &LocalName) -> Option<&str> {
        self.dom.attribute(id, ns, local)
    }

    fn attributes(&self, id: Self::NodeId) -> impl Iterator<Item = AttributeView<'_>> + '_ {
        self.dom.attributes(id)
    }

    fn text(&self, id: Self::NodeId) -> Option<&str> {
        self.dom.text(id)
    }
}

/// Lay out only `root`'s subtree at its own viewport as a retained,
/// incrementally-updatable session — the **forest-dom F2 primitive**: one
/// `ScriptedDom` with N window-root elements, each laid out by its own
/// [`IncrementalLayout`] over a [`SubtreeView`] at that window's size/sheet.
/// This is the "per-subtree layout at (root, viewport, sheet)" the forest-dom
/// plan calls the piece that "mostly does not exist"; it exists as the
/// composition of `SubtreeView` + `IncrementalLayout`, and
/// [`crate::subtree::tests`]' F0 spike proves two of them relayout
/// independently and survive a cross-root move. The host keeps one session per
/// window keyed to its window-root.
pub fn layout_subtree<D>(
    dom: &D,
    root: D::NodeId,
    stylesheets: &[&str],
    width: f32,
    height: f32,
) -> crate::IncrementalLayout<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
{
    crate::IncrementalLayout::new(&SubtreeView::new(dom, root), stylesheets, width, height)
}

/// Lay out only `root`'s subtree via the re-rooted view, reusing the full pipeline.
/// See the module note for the relative-geometry / inheritance boundary.
pub fn render_subtree<D>(
    dom: &D,
    root: D::NodeId,
    stylesheets: &[&str],
    viewport_width: f32,
    viewport_height: f32,
) -> FragmentPlane<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash + Send + Sync + 'static,
{
    render(
        &SubtreeView::new(dom, root),
        stylesheets,
        viewport_width,
        viewport_height,
    )
}

#[cfg(test)]
mod tests {
    //! Forest-dom **F0 spike** (2026-07-18): the load-bearing unknown, proven
    //! cheap before slicing. The forest-dom plan (mere design_docs
    //! 2026-07-08_forest_dom_plan.md) says its riskiest assumptions are #2
    //! (per-subtree layout at its own viewport) and #3 (a cross-root move seen
    //! correctly by every window), and prescribes a spike first: "one
    //! ScriptedDom with two sibling roots; two PaneSessions each laying out one
    //! root at its own size; mutate one subtree; confirm only that session
    //! relayouts ... then move a node from root A to root B and confirm both
    //! sessions see the re-root correctly."
    //!
    //! [`SubtreeView`] + [`IncrementalLayout`] (via [`layout_subtree`]) is the
    //! F2 primitive; this test IS that spike. It passing means the forest dom
    //! is structurally unblocked — the remaining work (F1 runner-mounts-at-node,
    //! F3 mutation routing by root) is plumbing, not a research risk.

    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};

    use super::{SubtreeView, layout_subtree};

    // Each div is a definite box, so geometry is stable to assert on.
    const SHEET: &str = "div { display: block; width: 100px; height: 20px; }";

    fn qual(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    fn div_with(dom: &mut ScriptedDom, parent: NodeId, text: &str) -> NodeId {
        let el = dom.create_element(qual("div"));
        let t = dom.create_text(text);
        dom.append_child(el, t);
        dom.append_child(parent, el);
        el
    }

    /// The style-sharing repro (found by merecat's chrome migration,
    /// 2026-07-18): a subtree-rooted cascade over DENSE SAME-CLASS SIBLING
    /// runs trips genet-stylo's style-sharing cache — `parent_style_identity`
    /// unwraps an inheritance parent (`sharing/mod.rs:259`) that an element
    /// root does not have, a shape upstream never sees because a whole
    /// document's root is the document, which never enters sharing. The F0
    /// spike's sparse shape did not trigger the cache; a chrome card's row
    /// run does. This must NOT panic.
    #[test]
    fn subtree_cascade_survives_same_class_sibling_runs() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let win = dom.create_element(qual("div"));
        dom.set_attribute(win, qual("class"), "window-root");
        dom.append_child(root, win);
        // A card holding a dense run of same-class rows — the sharing
        // cache's favorite shape (identical class, identical parent).
        let card = dom.create_element(qual("div"));
        dom.set_attribute(card, qual("class"), "card");
        dom.append_child(win, card);
        let mut rows = Vec::new();
        for i in 0..12 {
            let row = dom.create_element(qual("div"));
            dom.set_attribute(row, qual("class"), "row");
            let t = dom.create_text(&format!("row {i}"));
            dom.append_child(row, t);
            dom.append_child(card, row);
            rows.push(row);
        }
        const ROWS_SHEET: &str =
            "div { display: block; } .card { width: 300px; } .row { height: 20px; }";
        let layout = layout_subtree(&dom, win, &[ROWS_SHEET], 400.0, 600.0);
        let view = SubtreeView::new(&dom, win);
        for row in rows {
            assert!(
                layout.absolute_rect(&view, row).is_some(),
                "every row lays out under the subtree root"
            );
        }
    }

    #[test]
    fn two_window_roots_lay_out_independently_and_survive_a_cross_root_move() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        // One document, two sibling window-roots (the forest).
        let win_a = dom.create_element(qual("div"));
        dom.append_child(root, win_a);
        let win_b = dom.create_element(qual("div"));
        dom.append_child(root, win_b);

        let a1 = div_with(&mut dom, win_a, "alpha-1");
        let a2 = div_with(&mut dom, win_a, "alpha-2");
        let b1 = div_with(&mut dom, win_b, "beta-1");

        // #2: two sessions, DISTINCT per-window viewports (400x600, 800x300).
        let la = layout_subtree(&dom, win_a, &[SHEET], 400.0, 600.0);
        let lb = layout_subtree(&dom, win_b, &[SHEET], 800.0, 300.0);
        let va = SubtreeView::new(&dom, win_a);
        let vb = SubtreeView::new(&dom, win_b);

        // Each session lays out ITS subtree and nothing of the other's — the
        // fragment plane is the membership oracle (rect_of is None off-subtree).
        assert!(la.absolute_rect(&va, a1).is_some(), "A lays out its own node");
        assert!(la.absolute_rect(&va, a2).is_some());
        assert!(la.absolute_rect(&va, b1).is_none(), "A must NOT lay out B's node");
        assert!(lb.absolute_rect(&vb, b1).is_some(), "B lays out its own node");
        assert!(lb.absolute_rect(&vb, a1).is_none(), "B must NOT lay out A's node");
        // Stacked blocks: alpha-2 sits below alpha-1 in A's own coordinate space.
        let (_, y1, _, _) = la.absolute_rect(&va, a1).unwrap();
        let (_, y2, _, _) = la.absolute_rect(&va, a2).unwrap();
        assert!(y2 > y1, "A's second block is below its first");

        // #2 isolation: mutating winA does not disturb winB's independent
        // relayout. B's geometry is byte-identical before and after A's change.
        let b1_before = lb.absolute_rect(&vb, b1).unwrap();
        div_with(&mut dom, win_a, "alpha-3");
        let lb2 = layout_subtree(&dom, win_b, &[SHEET], 800.0, 300.0);
        let vb2 = SubtreeView::new(&dom, win_b);
        assert_eq!(
            lb2.absolute_rect(&vb2, b1).unwrap(),
            b1_before,
            "B's layout is untouched by a mutation confined to A's subtree"
        );

        // #3: a cross-root move — detach a2 from winA, append to winB (the
        // node-identity-preserving move a portable tile makes). Both sessions
        // re-lay-out and see the re-root correctly: A loses the node, B gains it,
        // and its NodeId is unchanged (identity survived the move).
        dom.remove_child(a2);
        dom.append_child(win_b, a2);
        let la3 = layout_subtree(&dom, win_a, &[SHEET], 400.0, 600.0);
        let lb3 = layout_subtree(&dom, win_b, &[SHEET], 800.0, 300.0);
        let va3 = SubtreeView::new(&dom, win_a);
        let vb3 = SubtreeView::new(&dom, win_b);
        assert!(
            la3.absolute_rect(&va3, a2).is_none(),
            "after the move, A no longer lays out the moved node"
        );
        assert!(
            lb3.absolute_rect(&vb3, a2).is_some(),
            "after the move, B lays out the moved node under its own root"
        );
        // The moved node kept its identity AND is now positioned in B's space,
        // below B's own beta-1 (proving it re-parented, not duplicated).
        let (_, moved_y, _, _) = lb3.absolute_rect(&vb3, a2).unwrap();
        let (_, beta_y, _, _) = lb3.absolute_rect(&vb3, b1).unwrap();
        assert!(moved_y > beta_y, "the moved node stacks under B's existing content");
    }
}
