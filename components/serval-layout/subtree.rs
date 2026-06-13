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

use crate::{render, FragmentPlane};

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
