/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Profile-neutral DOM trait.
//!
//! `LayoutDom` is the ID-first surface that `serval-layout` (and other
//! read-only DOM walkers — reader-mode, serialization, querySelector helpers)
//! consume. It does not commit to a backing store: `serval-static-dom`'s
//! `StaticDocument` and a future scripted-DOM provider both implement it.
//!
//! Design rationale and prior art: see
//! `docs/2026-05-16_layout_dom_api_design.md`.

#![deny(unsafe_code)]

use std::fmt::Debug;
use std::hash::Hash;
use std::ops::ControlFlow;

pub use markup5ever::{LocalName, Namespace, QualName};

/// Profile-neutral DOM. Implementors expose opaque `NodeId`s and a small set
/// of lookup primitives; traversal happens through the default `walk` impl
/// over a [`NodeVisitor`], or through caller-driven cursors built on the
/// lookup primitives.
pub trait LayoutDom {
    /// Opaque per-backend node identity. Must be `Copy` for cheap pass-through.
    type NodeId: Copy + Eq + Hash + Debug + 'static;

    // ---- identity / structure -------------------------------------------

    /// The document root.
    fn document(&self) -> Self::NodeId;

    /// Parent node, if any.
    fn parent(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// Previous sibling in DOM order. Hot on selector-matching paths
    /// (`prev_sibling_element` in `selectors::Element`); deriving it from
    /// `dom_children(parent)` would be O(siblings) per call.
    fn prev_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// Next sibling in DOM order. See [`Self::prev_sibling`].
    fn next_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// DOM-tree children (parse-order, ignores shadow trees).
    fn dom_children(&self, id: Self::NodeId)
        -> impl Iterator<Item = Self::NodeId> + '_;

    /// Flat-tree children (slot-assigned for shadow hosts, otherwise DOM
    /// order). Backends without shadow DOM should leave this defaulted.
    fn flat_children(&self, id: Self::NodeId)
        -> impl Iterator<Item = Self::NodeId> + '_
    {
        self.dom_children(id)
    }

    // ---- kind and hot primitives ----------------------------------------

    /// What kind of node `id` is. Plain enum; details via the typed
    /// accessors below.
    fn kind(&self, id: Self::NodeId) -> NodeKind;

    /// Stable per-node identity as a `u64`. Used by foreign trait adapters
    /// (Stylo's `OpaqueNode`, `selectors::OpaqueElement`) that need a
    /// pointer-shaped value for identity comparisons in the cascade.
    ///
    /// Must satisfy: distinct nodes within the same backing store return
    /// distinct `opaque_id` values, and the same node returns the same value
    /// across calls. Implementations may use the inner storage index (dense
    /// DOMs) or a hash (sparse DOMs).
    ///
    /// The default implementation hashes `id` with `DefaultHasher` — works
    /// for any `NodeId: Hash` but isn't guaranteed to be collision-free
    /// across all node sets. Backends should override when they can return
    /// the natural underlying index cheaply.
    fn opaque_id(&self, id: Self::NodeId) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        hasher.finish()
    }

    /// Element name when `id` is an element, else `None`. Hot on
    /// selector/style match paths.
    fn element_name(&self, id: Self::NodeId) -> Option<&QualName>;

    /// Attribute value lookup by namespace + local name. Hot on selector/style
    /// match paths. Backends with column-stored attrs can implement this as
    /// a keyed lookup without materializing a full slice.
    fn attribute(
        &self,
        id: Self::NodeId,
        ns: &Namespace,
        local: &LocalName,
    ) -> Option<&str>;

    /// Iterate this element's attributes (cold path: serialization,
    /// introspection). Yields `AttributeView`s borrowed from the backing
    /// store.
    fn attributes(&self, id: Self::NodeId)
        -> impl Iterator<Item = AttributeView<'_>> + '_;

    /// Text content for text or comment nodes, else `None`.
    fn text(&self, id: Self::NodeId) -> Option<&str>;

    // ---- traversal -------------------------------------------------------

    /// Walk the whole document from `document()`, descending via
    /// `dom_children`. Backends override when they want backend-driven
    /// traversal (parallel layout pass, prefetching, flat-tree descent).
    fn walk<V>(&self, visitor: &mut V) -> ControlFlow<V::Stop>
    where
        V: NodeVisitor<Self> + ?Sized,
    {
        walk_subtree(self, self.document(), visitor)
    }
}

/// Plain node kind. Use the typed accessors on [`LayoutDom`]
/// (`element_name`, `attribute`, `text`, etc.) to read kind-specific data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    Document,
    Doctype,
    Element,
    Text,
    Comment,
    ProcessingInstruction,
}

/// Borrowed view of one attribute on an element.
#[derive(Clone, Copy, Debug)]
pub struct AttributeView<'a> {
    pub name: &'a QualName,
    pub value: &'a str,
}

/// Visitor over a [`LayoutDom`]. Methods return [`ControlFlow`] so the visitor
/// can bail early with a typed `Stop` value. Use `type Stop = ()` for plain
/// "stop or not"; use `core::convert::Infallible` to assert the walk never
/// terminates early; use a typed error type to carry per-node-failure data
/// out of the walk.
pub trait NodeVisitor<D: LayoutDom + ?Sized> {
    /// Early-termination payload carried out of the walk.
    type Stop;

    /// Called when descending into a node. Default: descend.
    fn enter(
        &mut self,
        _dom: &D,
        _id: D::NodeId,
    ) -> ControlFlow<Self::Stop, Descent>
    {
        ControlFlow::Continue(Descent::Descend)
    }

    /// Called after a node's subtree has been visited. Default: continue.
    fn exit(
        &mut self,
        _dom: &D,
        _id: D::NodeId,
    ) -> ControlFlow<Self::Stop>
    {
        ControlFlow::Continue(())
    }
}

/// Per-node descent decision returned from [`NodeVisitor::enter`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Descent {
    /// Descend into this node's children.
    Descend,
    /// Skip this node's subtree but continue walking siblings/parent.
    Skip,
}

/// Walk `root`'s subtree with `visitor`, descending via
/// [`LayoutDom::dom_children`]. Returns `ControlFlow::Break(stop)` if any
/// visitor method bailed; otherwise `ControlFlow::Continue(())`.
pub fn walk_subtree<D, V>(
    dom: &D,
    root: D::NodeId,
    visitor: &mut V,
) -> ControlFlow<V::Stop>
where
    D: LayoutDom + ?Sized,
    V: NodeVisitor<D> + ?Sized,
{
    match visitor.enter(dom, root)? {
        Descent::Skip => ControlFlow::Continue(()),
        Descent::Descend => {
            for child in dom.dom_children(root) {
                walk_subtree(dom, child, visitor)?;
            }
            visitor.exit(dom, root)
        }
    }
}
